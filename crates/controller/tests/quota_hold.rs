//! Q53 / #828: durable quota holds must be visible and wake on evidence.
#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    fs,
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant, UNIX_EPOCH},
};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue,
    adapters::{
        self,
        workers::{Supervisor, WorkerOutputSource},
    },
    cli::Input,
    executor::{Completion, Delivery, Executor, Observation, Output, Reconciliation, Upstream},
    status,
};
use tl3_kernel::{EffectKind, Phase, Role};
use tl3_store::{Commit, Reader, RowChange, Store};

const RESET: u64 = 1_900_000_000_000;
#[derive(Clone)]
struct Clock(Rc<Cell<u64>>);
impl tl3_controller::clock::Clock for Clock {
    fn now(&self) -> std::time::SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.0.get())
    }
}

// Only audit uses a real worker. Planning stays in flight so these tests cannot
// accidentally finish a request while checking its audit ending.
struct Audit(
    Supervisor,
    adapters::workers::Fake,
    Rc<Cell<bool>>,
    Rc<Cell<bool>>,
    Rc<RefCell<Option<Observation>>>,
);
impl Executor for Audit {
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        match d.output {
            Output::Effect {
                kind: EffectKind::StartRun(Role::Audit),
                ..
            } => self.0.execute(d),
            Output::Effect {
                kind: EffectKind::StartRun(Role::Plan),
                ..
            } => Ok(Completion::confirmed(vec![])),
            _ => self.1.execute(d),
        }
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        match d.output {
            Output::Effect {
                kind: EffectKind::StartRun(Role::Audit),
                ..
            } => {
                let result = self.0.reconcile(d)?;
                if let Reconciliation::Found(completion) = &result {
                    if let Some(reply) = completion
                        .observations
                        .iter()
                        .find(|o| matches!(o, Observation::WorkerReply { .. }))
                    {
                        *self.4.borrow_mut() = Some(reply.clone());
                        if self.3.get() {
                            return Ok(Reconciliation::Pending);
                        }
                    }
                }
                if self.2.get()
                    && matches!(&result, Reconciliation::Found(c) if c.observations.iter().any(|o| matches!(o, Observation::Quota { .. })))
                {
                    Ok(Reconciliation::Pending)
                } else {
                    Ok(result)
                }
            }
            Output::Effect {
                kind: EffectKind::StartRun(Role::Plan),
                ..
            } => Ok(Reconciliation::Pending),
            _ => self.1.reconcile(d),
        }
    }
    fn quota_resume_ms(
        &mut self,
        d: &Delivery,
        reset: u64,
        now: u64,
    ) -> tl3_controller::Result<u64> {
        self.0.quota_resume_ms(d, reset, now)
    }
    fn quota_candidates(
        &mut self,
        d: &Delivery,
    ) -> tl3_controller::Result<Vec<tl3_controller::looks::QuotaModel>> {
        self.0.quota_candidates(d)
    }
    fn resume_after_quota(&mut self, d: &Delivery) -> tl3_controller::Result<Option<Completion>> {
        self.0.resume_after_quota(d)
    }
}

struct Fixture {
    _db: support::Database,
    root: tempfile::TempDir,
    settings: tl3_config::Store,
    controller: Controller,
    source: WorkerOutputSource,
    store: Store,
    clock: Clock,
    config: tl3_config::Config,
    defer_quota: Rc<Cell<bool>>,
    deferred: Vec<(String, Observation)>,
    success_reply: Option<Observation>,
    defer_success: Rc<Cell<bool>>,
    observed_success: Rc<RefCell<Option<Observation>>>,
    allow_quota_incident: bool,
}

impl Fixture {
    fn new() -> Self {
        let mut db = support::Database::start();
        let settings = db.create("quota_hold");
        let root = tempfile::tempdir().unwrap();
        let mut config =
            tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        config.store = settings.clone();
        config.limits.agent_pool = 2;
        // These tests advance the quota clock by days; unrelated simulated
        // workers have no silence/no-progress deadline in this fixture.
        config.allowances.agent_worker.silence_secs = u32::MAX;
        config.allowances.agent_worker.no_progress_secs = u32::MAX;
        config.models.key_file = root.path().join("fixture-key");
        fs::write(&config.models.key_file, "fixture-only").unwrap();
        config.workers.python = "/usr/bin/python3".into();
        config.workers.runs = root.path().join("runs");
        // Raw upstream facts only. The real supervisor parses and persists the
        // ending; the real controller decides the quota hold and redispatch.
        config.workers.command = vec!["-c".into(), r#"
import json, pathlib, sys, time
r = json.load(sys.stdin)
root = pathlib.Path(sys.argv[1])
node = r['operation']['node']
attempt = pathlib.Path(r['trace']['completion']).parent
(root / ('started-' + str(node))).write_text(str(attempt))
if node == 1 and not (root / 'quota-all').exists():
    while not (root / 'success').exists():
        time.sleep(0.01)
if attempt.name != 'attempt-00000000' and (root / 'probe-block').exists():
    while not (root / 'probe-release').exists():
        time.sleep(0.01)
quota = (node == 0 or (root / 'quota-all').exists()) and not (root / 'restored').exists()
now = int(time.time() * 1000)
facts = dict(started_ms=now, finished_ms=now, outcome='quota' if quota else 'succeeded', release=None,
             model=r['model']['name'], input_tokens=None, cached_tokens=None, output_tokens=None,
             trace_id=None, failure=None)
pathlib.Path(r['trace']['completion']).write_text(json.dumps(facts))
if quota:
    print(json.dumps(dict(event='quota', role='audit', reset_ms=1900000000000)), flush=True)
else:
    print(json.dumps(dict(event='reply', role='audit', reply=dict(verdict='agree', objections=[], evidence_contract=[]))), flush=True)
"#.into(), root.path().to_str().unwrap().into()];
        let clock = Clock(Rc::new(Cell::new(RESET - 4 * 86_400_000)));
        let mut registry = adapters::simulation(&root.path().join("fakes")).unwrap();
        let mut ownership =
            tl3_controller::Ownership::claim(Store::open(&settings).unwrap()).unwrap();
        let supervisor =
            Supervisor::new(&config, &root.path().join("records"), &mut ownership).unwrap();
        let source = supervisor.source();
        let defer_quota = Rc::new(Cell::new(false));
        let defer_success = Rc::new(Cell::new(false));
        let observed_success = Rc::new(RefCell::new(None));
        registry.register(
            Upstream::Workers,
            Audit(
                supervisor,
                adapters::workers::Fake::new(root.path()),
                defer_quota.clone(),
                defer_success.clone(),
                observed_success.clone(),
            ),
        );
        let mut controller = Controller::open_with_clock(
            ownership.into_store(),
            registry,
            Bootstrap::from_config(&config).unwrap(),
            clock.clone(),
        )
        .unwrap();
        controller.set_allowances(config.allowances.clone());
        controller
            .set_model_quota_policy(&config.models, Duration::from_secs(6 * 3600))
            .unwrap();
        let mut store = Store::open(&settings).unwrap();
        let snapshot = store.load().unwrap();
        let mut commit = Commit::new("fixture:briefs", json!({}), &snapshot);
        for node in ["0", "1"] {
            store.put_worker_materials(node, &json!({"issue":{"number":42,"title":"quota wait","body":"audit","author":"requester","comments":[]},"repo":{"readme":"fixture","file_list":["greeting.py"]}})).unwrap();
            commit.rows.push(RowChange::Put { table:"briefs".into(), id:node.into(), value:json!({"kind":"brief","brief":"audit","goals":[{"description":"Audit the greeting.","verification":"test"}],"files":["greeting.py"],"memory_ids":[],"evidence_contract":[]}) });
        }
        store.commit(&commit).unwrap();
        Self {
            _db: db,
            root,
            settings,
            controller,
            source,
            store,
            clock,
            config,
            defer_quota,
            deferred: vec![],
            success_reply: None,
            defer_success,
            observed_success,
            allow_quota_incident: false,
        }
    }
    fn intake(&mut self, number: u64) {
        self.controller
            .intake(LabelledIssue {
                delivery: format!("fixture-{number}"),
                owner: "RobTand".into(),
                repo: "tl3-sandbox".into(),
                installation: 1,
                number,
                title: "quota wait".into(),
                actor: 24923402,
                label: "tl3".into(),
                labels: vec![],
            })
            .unwrap();
        self.drain();
    }
    fn drain(&mut self) {
        for _ in 0..300 {
            if !self.controller.drive_once().unwrap() {
                return;
            }
        }
        let mut reader = Reader::open(&self.settings).unwrap();
        let mut tx = reader.transaction().unwrap();
        let rows: Vec<_> = tx
            .query(
                "SELECT event_key, payload FROM tl3_events ORDER BY revision DESC LIMIT 4",
                &[],
            )
            .unwrap()
            .into_iter()
            .map(|row| (row.get::<_, String>(0), row.get::<_, serde_json::Value>(1)))
            .collect();
        let pending: Vec<_> = self
            .controller
            .snapshot()
            .unwrap()
            .rows
            .into_iter()
            .filter(|row| {
                row.table == "inbox"
                    || row.table == "allowances"
                    || row.table == "model_quota_probes"
            })
            .collect();
        panic!("controller did not settle: {rows:?}; pending {pending:?}");
    }
    fn step(&mut self) {
        let (tx, rx) = mpsc::channel();
        self.source.poll_once(&mut self.store, &tx).unwrap();
        for input in rx.try_iter() {
            if let Input::Observation { key, observation } = input {
                if self.defer_quota.get() && matches!(observation, Observation::Quota { .. }) {
                    self.deferred.push((key, observation));
                    continue;
                }
                if matches!(
                    observation,
                    Observation::WorkerReply {
                        node: tl3_kernel::NodeId(1),
                        ..
                    }
                ) {
                    self.success_reply = Some(observation.clone());
                    if self.defer_success.get() {
                        self.deferred.push((key, observation));
                        continue;
                    }
                }
                self.controller.observe(&key, observation).unwrap();
            }
        }
        self.drain();
        let incidents: Vec<_> = self
            .controller
            .snapshot()
            .unwrap()
            .rows
            .into_iter()
            .filter(|r| {
                r.table == "incidents"
                    && !(self.allow_quota_incident
                        && r.value["signature"]["class"] == "model_quota_hint_expired")
            })
            .collect();
        assert!(incidents.is_empty(), "fixture incident: {incidents:?}");
    }
    fn until(&mut self, mut ready: impl FnMut(&mut Self) -> bool) {
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            self.step();
            if ready(self) {
                return;
            }
            assert!(Instant::now() < end, "worker ending did not arrive");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn hold(&mut self) -> Option<(String, serde_json::Value)> {
        self.hold_for(0)
    }
    fn hold_for(&mut self, node: u64) -> Option<(String, serde_json::Value)> {
        self.controller
            .snapshot()
            .unwrap()
            .rows
            .into_iter()
            .find(|r| {
                r.table == "allowances"
                    && r.value["delivery"]["output"]["Effect"]["node"] == node
                    && r.value["quota_ms"].as_u64().is_some()
            })
            .map(|r| (r.id, r.value))
    }
    fn held(&mut self, other: bool) -> String {
        self.intake(2662);
        if other {
            self.intake(2663);
        }
        self.until(|f| f.hold().is_some() && (!other || f.root.path().join("started-1").exists()));
        self.controller.look_due().unwrap();
        self.hold().unwrap().0
    }
    fn attempt(&mut self, key: &str) -> i64 {
        self.store
            .worker_progress(None)
            .unwrap()
            .into_iter()
            .find(|p| p.key == key)
            .unwrap()
            .attempt
    }
    fn success(&mut self) {
        fs::write(self.root.path().join("restored"), "yes").unwrap();
        fs::write(self.root.path().join("success"), "yes").unwrap();
        self.until(|f| f.controller.world().unwrap().nodes[1].phase != Phase::Auditing);
        assert!(
            self.controller
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .all(|r| r.table != "model_quotas"),
            "the real successful reply clears the model quota record"
        );
    }
    fn restart(self) -> Self {
        let Self {
            _db,
            root,
            settings,
            controller,
            source,
            store,
            clock,
            config,
            defer_quota,
            deferred,
            success_reply,
            defer_success,
            observed_success,
            allow_quota_incident,
        } = self;
        drop(controller);
        drop(source);
        let mut reader = Reader::open(&settings).unwrap();
        let mut tx = reader.transaction().unwrap();
        tx.query_one("SELECT pg_advisory_lock($1)", &[&0x544c33434f4e5452_i64])
            .unwrap();
        tx.query_one("SELECT pg_advisory_unlock($1)", &[&0x544c33434f4e5452_i64])
            .unwrap();
        tx.commit().unwrap();
        let mut registry = adapters::simulation(&root.path().join("fakes")).unwrap();
        let mut ownership =
            tl3_controller::Ownership::claim(Store::open(&settings).unwrap()).unwrap();
        let supervisor =
            Supervisor::new(&config, &root.path().join("records"), &mut ownership).unwrap();
        let source = supervisor.source();
        registry.register(
            Upstream::Workers,
            Audit(
                supervisor,
                adapters::workers::Fake::new(root.path()),
                defer_quota.clone(),
                defer_success.clone(),
                observed_success.clone(),
            ),
        );
        let mut controller = Controller::open_with_clock(
            ownership.into_store(),
            registry,
            Bootstrap::from_config(&config).unwrap(),
            clock.clone(),
        )
        .unwrap();
        controller.set_allowances(config.allowances.clone());
        controller
            .set_model_quota_policy(&config.models, Duration::from_secs(6 * 3600))
            .unwrap();
        Self {
            _db,
            root,
            settings,
            controller,
            source,
            store,
            clock,
            config,
            defer_quota,
            deferred,
            success_reply,
            defer_success,
            observed_success,
            allow_quota_incident,
        }
    }
}

#[test]
fn quota_status_names_the_durable_hold_and_retry() {
    let mut f = Fixture::new();
    f.controller
        .set_model_quota_policy(&f.config.models, Duration::from_secs(5 * 86_400))
        .unwrap();
    let key = f.held(false);
    let (_, hold) = f.hold().unwrap();
    assert_eq!(hold["quota_ms"], RESET);
    let config = tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
    let holds = tl3_controller::looks::model_quota_holds(
        &f.controller.snapshot().unwrap(),
        Some(&config.models),
    )
    .unwrap();
    let facts = holds.iter().find(|hold| hold.key == key).unwrap();
    assert_eq!(facts.role, Role::Audit);
    assert_eq!(facts.model.as_deref(), Some("alternate-model"));
    assert_eq!(facts.family.as_deref(), Some("alternate"));
    assert_eq!(facts.reset_ms, RESET);
    assert_eq!(
        facts.retry_at_ms, RESET,
        "the maintenance look is not a retry deadline"
    );
    let mut reader = Reader::open(&f.settings).unwrap();
    let view = status::load(&mut reader).unwrap();
    let wait = &view.requests[0].waiting_on;
    assert!(
        wait.contains("model quota (audit, alternate-model)"),
        "quota hold is hidden: {wait}"
    );
    assert!(
        wait.contains("2030-03-17T17:46:40Z"),
        "reset missing: {wait}"
    );
    assert!(wait.contains("earliest retry"), "retry missing: {wait}");
    assert!(status::render_text(&view).contains(wait));
    assert!(status::render_html(&view).contains(wait));
    f.clock.0.set(RESET - 1);
    f.controller.look_due().unwrap();
    assert_eq!(f.attempt(&key), 0, "must wait until the announced reset");
    fs::write(f.root.path().join("restored"), "yes").unwrap();
    f.clock.0.set(RESET);
    f.controller.look_due().unwrap();
    assert_eq!(f.attempt(&key), 1, "the original key resumes at reset");
    f.until(|f| f.controller.world().unwrap().nodes[0].phase != Phase::Auditing);
}

#[test]
fn duplicate_success_and_reset_do_not_dispatch_twice() {
    let mut f = Fixture::new();
    let key = f.held(true);
    f.success();
    f.controller.look_due().unwrap();
    assert_eq!(f.attempt(&key), 1);
    let reply = f
        .success_reply
        .clone()
        .or_else(|| f.observed_success.borrow().clone())
        .expect("real successful ending");
    for suffix in ["duplicate-1", "duplicate-2"] {
        f.controller.observe(suffix, reply.clone()).unwrap();
        f.drain();
        f.controller.look_due().unwrap();
    }
    f.clock.0.set(RESET);
    f.controller.look_due().unwrap();
    f.controller.look_due().unwrap();
    assert_eq!(
        f.attempt(&key),
        1,
        "success duplicates and the reset share one attempt"
    );
}

#[test]
fn same_model_success_redispatches_the_quota_held_key() {
    let mut f = Fixture::new();
    let key = f.held(true);
    f.success();
    f.controller.look_due().unwrap();
    assert_eq!(
        f.attempt(&key),
        1,
        "a later same-model success must redispatch the held idempotency key before reset"
    );
}

#[test]
fn restart_between_success_and_redispatch_keeps_the_wake() {
    let mut f = Fixture::new();
    let key = f.held(true);
    f.success();
    assert_eq!(f.attempt(&key), 0, "success persisted before redispatch");
    let mut f = f.restart();
    f.controller.look_due().unwrap();
    assert_eq!(f.attempt(&key), 1, "restart must preserve the success wake");
    f.controller.look_due().unwrap();
    assert_eq!(f.attempt(&key), 1);
}

#[test]
fn success_before_quota_is_durable_does_not_park_it() {
    let mut f = Fixture::new();
    f.defer_quota.set(true);
    f.intake(2662);
    f.intake(2663);
    f.until(|f| !f.deferred.is_empty() && f.root.path().join("started-1").exists());
    let key = match &f.deferred[0].1 {
        Observation::Quota { key, .. } => key.clone(),
        _ => unreachable!(),
    };
    f.success();
    f.defer_quota.set(false);
    for (key, observation) in std::mem::take(&mut f.deferred) {
        f.controller.observe(&key, observation).unwrap();
    }
    f.drain();
    f.controller.look_due().unwrap();
    assert_eq!(
        f.attempt(&key),
        1,
        "a delayed quota ending cannot overwrite later success"
    );
}

#[test]
fn far_reset_uses_one_hourly_probe_per_model_and_family() {
    let mut f = Fixture::new();
    fs::write(f.root.path().join("quota-all"), "yes").unwrap();
    fs::write(f.root.path().join("probe-block"), "yes").unwrap();
    f.intake(2662);
    f.intake(2663);
    f.until(|f| f.hold_for(0).is_some() && f.hold_for(1).is_some());
    f.controller.look_due().unwrap();
    let key0 = f.hold_for(0).unwrap().0;
    let key1 = f.hold_for(1).unwrap().0;
    let hour = f.clock.0.get() + 3_600_000;
    f.clock.0.set(hour - 1);
    f.controller.look_due().unwrap();
    assert_eq!(f.attempt(&key0) + f.attempt(&key1), 0);
    f.clock.0.set(hour);
    f.controller.look_due().unwrap();
    assert_eq!(
        f.attempt(&key0) + f.attempt(&key1),
        1,
        "one probe for the shared model and family"
    );
    f.clock.0.set(hour + 2 * 3_600_000);
    f.controller.look_due().unwrap();
    assert_eq!(
        f.attempt(&key0) + f.attempt(&key1),
        1,
        "the outstanding probe fences later hours"
    );
    f.clock.0.set(RESET);
    f.controller.look_due().unwrap();
    assert_eq!(
        f.attempt(&key0) + f.attempt(&key1),
        1,
        "reset cannot bypass an outstanding probe"
    );
    fs::write(f.root.path().join("restored"), "yes").unwrap();
    fs::write(f.root.path().join("probe-release"), "yes").unwrap();
    f.until(|f| {
        f.controller
            .world()
            .unwrap()
            .nodes
            .iter()
            .any(|n| n.phase != Phase::Auditing)
    });
}

/// Q53 / #828: a real late success is availability evidence after its run was stopped.
#[test]
fn late_success_wakes_another_request_after_its_node_moved_on() {
    let mut f = Fixture::new();
    let key = f.held(true);
    f.defer_success.set(true);
    fs::write(f.root.path().join("restored"), "yes").unwrap();
    fs::write(f.root.path().join("success"), "yes").unwrap();
    f.until(|f| !f.deferred.is_empty());
    f.controller
        .observe(
            "stop-success-node",
            Observation::Steering {
                node: tl3_kernel::NodeId(1),
                command: tl3_kernel::Command::Stop,
            },
        )
        .unwrap();
    f.drain();
    assert_ne!(
        f.controller.world().unwrap().nodes[1].phase,
        Phase::Auditing
    );
    for (key, observation) in std::mem::take(&mut f.deferred) {
        f.controller.observe(&key, observation).unwrap();
    }
    f.drain();
    f.controller.look_due().unwrap();
    assert_eq!(
        f.attempt(&key),
        1,
        "late availability evidence wakes the held key"
    );
}

/// Q53 / #828: retrying the same hint is a new ending, with an hourly shared budget.
#[test]
fn failed_probes_repeat_hourly_and_report_a_missed_reset() {
    let mut f = Fixture::new();
    let key = f.held(false);
    let start = f.clock.0.get();
    f.clock.0.set(start + 3_600_000);
    f.controller.look_due().unwrap();
    assert_eq!(f.attempt(&key), 1);
    f.until(|f| {
        f.hold()
            .is_some_and(|(_, state)| state["last_quota_attempt"] == 1)
    });
    f.controller.look_due().unwrap();
    f.clock.0.set(start + 7_199_999);
    f.controller.look_due().unwrap();
    assert_eq!(
        f.attempt(&key),
        1,
        "a failed probe cannot reset the hourly budget"
    );
    f.clock.0.set(start + 7_200_000);
    f.controller.look_due().unwrap();
    assert_eq!(f.attempt(&key), 2);
    f.until(|f| {
        f.hold()
            .is_some_and(|(_, state)| state["last_quota_attempt"] == 2)
    });
    f.controller.look_due().unwrap();
    f.clock.0.set(RESET + 1);
    f.allow_quota_incident = true;
    f.controller.look_due().unwrap();
    assert_eq!(f.attempt(&key), 3);
    f.until(|f| {
        f.hold()
            .is_some_and(|(_, state)| state["last_quota_attempt"] == 3)
    });
    f.controller.look_due().unwrap();
    let incidents: Vec<_> = f
        .controller
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .filter(|row| {
            row.table == "incidents"
                && row.value["signature"]["class"] == "model_quota_hint_expired"
        })
        .collect();
    assert_eq!(
        incidents.len(),
        1,
        "failed probe past hint raises one infrastructure incident"
    );
    assert_eq!(incidents[0].value["cause"], "upstream");
    f.controller.look_due().unwrap();
    assert_eq!(
        f.attempt(&key),
        3,
        "past-reset failures keep the hourly bound"
    );
}

/// Q53 / #828: process restart retains the in-flight model/family probe fence.
#[test]
fn restart_does_not_overlap_a_running_probe() {
    let mut f = Fixture::new();
    fs::write(f.root.path().join("quota-all"), "yes").unwrap();
    fs::write(f.root.path().join("probe-block"), "yes").unwrap();
    f.intake(2662);
    f.intake(2663);
    f.until(|f| f.hold_for(0).is_some() && f.hold_for(1).is_some());
    f.controller.look_due().unwrap();
    let key0 = f.hold_for(0).unwrap().0;
    let key1 = f.hold_for(1).unwrap().0;
    f.clock.0.set(f.clock.0.get() + 3_600_000);
    f.controller.look_due().unwrap();
    assert_eq!(f.attempt(&key0) + f.attempt(&key1), 1);
    let mut f = f.restart();
    f.clock.0.set(f.clock.0.get() + 2 * 3_600_000);
    f.controller.look_due().unwrap();
    assert_eq!(f.attempt(&key0) + f.attempt(&key1), 1);
    fs::write(f.root.path().join("restored"), "yes").unwrap();
    fs::write(f.root.path().join("probe-release"), "yes").unwrap();
    f.until(|f| {
        f.controller
            .world()
            .unwrap()
            .nodes
            .iter()
            .any(|node| node.phase != Phase::Auditing)
    });
}

/// Q53 / #828: a configured family change cannot relabel an older hold's evidence.
#[test]
fn success_in_another_family_does_not_clear_the_old_hold() {
    let mut f = Fixture::new();
    let key = f.held(false);
    for ladder in [
        &mut f.config.models.ingest,
        &mut f.config.models.audit,
        &mut f.config.models.planner,
        &mut f.config.models.maker,
        &mut f.config.models.reviewer,
        &mut f.config.models.rebaser,
        &mut f.config.models.comment_classifier,
        &mut f.config.models.writer,
        &mut f.config.models.postmortem,
    ] {
        for model in ladder {
            if model.name == "alternate-model" {
                model.family = "replacement-family".into();
            }
        }
    }
    let mut f = f.restart();
    f.intake(2663);
    fs::write(f.root.path().join("restored"), "yes").unwrap();
    fs::write(f.root.path().join("success"), "yes").unwrap();
    f.until(|f| f.controller.world().unwrap().nodes[1].phase != Phase::Auditing);
    f.controller.look_due().unwrap();
    assert_eq!(
        f.attempt(&key),
        0,
        "same name in another family is not wake evidence"
    );
    let holds = tl3_controller::looks::model_quota_holds(
        &f.controller.snapshot().unwrap(),
        Some(&f.config.models),
    )
    .unwrap();
    assert_eq!(
        holds
            .iter()
            .find(|hold| hold.key == key)
            .unwrap()
            .family
            .as_deref(),
        Some("alternate")
    );
}
