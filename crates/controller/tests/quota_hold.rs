//! Q53 / #828: durable quota holds must be visible and wake on evidence.
#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::json;
use std::{
    cell::Cell,
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
    executor::{Completion, Delivery, Executor, Output, Reconciliation, Upstream},
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
struct Audit(Supervisor, adapters::workers::Fake);
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
            } => self.0.reconcile(d),
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
}

impl Fixture {
    fn new() -> Self {
        let mut db = support::Database::start();
        let settings = db.create("quota_hold");
        let root = tempfile::tempdir().unwrap();
        let mut config =
            tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        config.store = settings.clone();
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
if node == 1:
    while not (root / 'success').exists():
        time.sleep(0.01)
quota = node == 0 and not (root / 'restored').exists()
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
        registry.register(
            Upstream::Workers,
            Audit(supervisor, adapters::workers::Fake::new(root.path())),
        );
        let controller = Controller::open_with_clock(
            ownership.into_store(),
            registry,
            Bootstrap::from_config(&config).unwrap(),
            clock.clone(),
        )
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
        panic!("controller did not settle");
    }
    fn step(&mut self) {
        let (tx, rx) = mpsc::channel();
        self.source.poll_once(&mut self.store, &tx).unwrap();
        for input in rx.try_iter() {
            if let Input::Observation { key, observation } = input {
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
            .filter(|r| r.table == "incidents")
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
        self.controller
            .snapshot()
            .unwrap()
            .rows
            .into_iter()
            .find(|r| {
                r.table == "allowances"
                    && r.value["delivery"]["output"]["Effect"]["node"] == 0
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
}

#[test]
fn quota_status_names_the_durable_hold_and_retry() {
    let mut f = Fixture::new();
    let key = f.held(false);
    let (_, hold) = f.hold().unwrap();
    assert_eq!(hold["quota_ms"], RESET);
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
#[ignore = "expected red pending Q27 #828: early wake on same-model success"]
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
