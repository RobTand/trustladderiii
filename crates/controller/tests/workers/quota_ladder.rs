//! Q53: a quota-limited step falls back down its role's ladder.
use super::*;

#[derive(Clone)]
struct Clock(std::rc::Rc<std::cell::Cell<u64>>);
impl tl3_controller::clock::Clock for Clock {
    fn now(&self) -> std::time::SystemTime {
        std::time::UNIX_EPOCH + Duration::from_millis(self.0.get())
    }
}

struct Fixture {
    root: tempfile::TempDir,
    config: tl3_config::Config,
    clock: Clock,
    controller: Controller,
    source: WorkerOutputSource,
    store: Store,
}

fn real_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn open(
    settings: &tl3_config::Store,
    root: &Path,
    config: &tl3_config::Config,
    clock: &Clock,
) -> (Controller, WorkerOutputSource) {
    let (registry, source) = worker_simulation(&root.join("fakes"), config, false).unwrap();
    let controller = Controller::open_with_clock(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap::from_config(config).unwrap(),
        clock.clone(),
    )
    .unwrap();
    (controller, source)
}

fn fixture(settings: &tl3_config::Store, planner: &[&str], limits: Value) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(settings, root.path(), "quota-ladder");
    let model = |name: &str| {
        [&config.models.ingest[0], &config.models.audit[0]]
            .into_iter()
            .find(|m| m.name == name)
            .unwrap()
            .clone()
    };
    config.models.planner = planner.iter().map(|name| model(name)).collect();
    fs::write(root.path().join("entered.quota-models"), limits.to_string()).unwrap();
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let clock = Clock(std::rc::Rc::new(std::cell::Cell::new(real_now())));
    let (mut controller, source) = open(settings, root.path(), &config, &clock);
    controller.intake(labelled()).unwrap();
    Fixture {
        root,
        config,
        clock,
        controller,
        source,
        store,
    }
}

impl Fixture {
    fn step(&mut self) {
        drive(&mut self.controller, &mut self.source, &mut self.store);
    }
    fn planner_models(&self) -> Vec<String> {
        if !self.root.path().join("entered.requests").exists() {
            return vec![];
        }
        recorded_runs(self.root.path())
            .into_iter()
            .filter(|r| r["request"]["role"] == "planner")
            .map(|r| r["request"]["model"]["name"].as_str().unwrap().to_owned())
            .collect()
    }
    fn quota_waits(&mut self) -> Vec<Value> {
        self.controller
            .snapshot()
            .unwrap()
            .rows
            .into_iter()
            .filter(|r| r.table == "allowances" && !r.value["quota_ms"].is_null())
            .map(|r| r.value)
            .collect()
    }
    fn planner_attempt(&mut self) -> i64 {
        let snapshot = self.controller.snapshot().unwrap();
        let key = snapshot
            .rows
            .iter()
            .find(|r| {
                r.table == "allowances"
                    && r.value["delivery"]["output"]["Effect"]["kind"]["StartRun"] == "Plan"
            })
            .map(|r| r.id.clone())
            .expect("planner run is watched");
        self.store
            .worker_progress(None)
            .unwrap()
            .into_iter()
            .find(|p| p.key == key)
            .expect("planner run was claimed")
            .attempt
    }
    /// Wait until the planner run on this model ended with quota and the controller recorded it.
    fn quota_recorded(&mut self, model: &str) {
        wait(|| {
            self.step();
            self.planner_models()
                .last()
                .is_some_and(|last| last == model)
                && !self.quota_waits().is_empty()
                && !self
                    .controller
                    .snapshot()
                    .unwrap()
                    .rows
                    .iter()
                    .any(|r| r.table == "inbox")
        });
    }
    fn finish_planning(&mut self) {
        wait(|| {
            self.step();
            self.controller.look_due().unwrap();
            !matches!(
                self.controller.world().unwrap().nodes[0].phase,
                Phase::Ingesting | Phase::Auditing | Phase::Unplanned | Phase::Planning { .. }
            )
        });
        let node = self.controller.world().unwrap().nodes[0];
        assert_eq!(node.run_failures, 0, "quota is not a failure");
        assert_eq!(node.plan_retries, 0, "quota spends no planning retry");
    }
    /// Wait until the planner run on this model ended, and say whether it
    /// ended like quota rather than as a failed run.
    fn planner_ended(&mut self, model: &str) -> bool {
        wait(|| {
            self.step();
            let node = self.controller.world().unwrap().nodes[0];
            self.planner_models()
                .last()
                .is_some_and(|last| last == model)
                && (!self.quota_waits().is_empty()
                    || node.run_failures > 0
                    || node.run_incidents > 0)
                && !self
                    .controller
                    .snapshot()
                    .unwrap()
                    .rows
                    .iter()
                    .any(|r| r.table == "inbox")
        });
        !self.quota_waits().is_empty()
    }
    fn model_quota(&mut self, model: &str) -> Option<u64> {
        self.controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .find(|r| r.table == "model_quotas" && r.id == model)
            .and_then(|r| r.value["reset_ms"].as_u64())
    }
}

pub(super) fn planner_falls_back(settings: &tl3_config::Store) {
    let mut f = fixture(
        settings,
        &["default-model", "alternate-model"],
        json!({"planner": {"default-model": 600}}),
    );
    f.quota_recorded("default-model");
    f.controller.look_due().unwrap();
    assert_eq!(
        f.planner_attempt(),
        1,
        "the next model starts at once, without waiting for the reset"
    );
    f.finish_planning();
    assert_eq!(f.planner_models(), ["default-model", "alternate-model"]);
    assert!(
        f.model_quota("default-model")
            .is_some_and(|reset| reset > real_now()),
        "the limited model is recorded with its reset"
    );
    let mut reader = tl3_store::Reader::open(settings).unwrap();
    let status = tl3_controller::status::load(&mut reader).unwrap();
    assert!(
        status
            .quota_models
            .iter()
            .any(|note| note.contains("default-model")),
        "the status view notes the quota-limited model: {:?}",
        status.quota_models
    );
}

pub(super) fn whole_ladder_waits(settings: &tl3_config::Store) {
    let mut f = fixture(
        settings,
        &["default-model", "alternate-model"],
        json!({"planner": {"default-model": 600, "alternate-model": 300}}),
    );
    f.quota_recorded("default-model");
    f.controller.look_due().unwrap();
    assert_eq!(f.planner_attempt(), 1, "falls back to the second model");
    f.quota_recorded("alternate-model");
    f.controller.look_due().unwrap();
    assert_eq!(
        f.planner_attempt(),
        1,
        "every model is out, so the step waits"
    );
    let earliest = f.model_quota("alternate-model").unwrap();
    assert!(earliest < f.model_quota("default-model").unwrap());
    let due: Vec<_> = f
        .quota_waits()
        .iter()
        .filter_map(|w| w["due_ms"].as_u64())
        .collect();
    assert_eq!(due, [earliest], "the wait wakes at the earliest reset");
    f.clock.0.set(earliest - 1);
    f.controller.look_due().unwrap();
    assert_eq!(f.planner_attempt(), 1);
    // The provider restored the model at its announced reset.
    fs::write(
        f.root.path().join("entered.quota-models"),
        json!({"planner": {"default-model": 600}}).to_string(),
    )
    .unwrap();
    f.clock.0.set(earliest);
    f.controller.look_due().unwrap();
    assert_eq!(f.planner_attempt(), 2, "the reset wakes the step");
    f.finish_planning();
    assert_eq!(
        f.planner_models(),
        ["default-model", "alternate-model", "alternate-model"]
    );
}

pub(super) fn removed_model_resumes_on_open(settings: &tl3_config::Store, legacy: bool) {
    let mut f = fixture(
        settings,
        &["default-model"],
        json!({"planner": {"default-model": 600}}),
    );
    f.quota_recorded("default-model");
    f.controller.look_due().unwrap();
    assert_eq!(f.planner_attempt(), 0, "a one-model ladder waits");
    if legacy {
        // Waits recorded before Q53 have no per-model record.
        let snapshot = f.controller.snapshot().unwrap();
        let mut commit =
            tl3_store::Commit::new("legacy-quota-wait", json!({"legacy": true}), &snapshot);
        for row in snapshot.rows.iter().filter(|r| r.table == "model_quotas") {
            commit.rows.push(tl3_store::RowChange::Delete {
                table: row.table.clone(),
                id: row.id.clone(),
            });
        }
        if !commit.rows.is_empty() {
            f.store.commit(&commit).unwrap();
        }
    }
    let Fixture {
        root,
        mut config,
        clock,
        controller,
        source,
        store,
    } = f;
    drop(controller);
    drop(source);
    config.models.planner = vec![config.models.audit[0].clone()];
    let (mut controller, source) = open(settings, root.path(), &config, &clock);
    let mut f = Fixture {
        root,
        config,
        clock,
        controller: {
            controller.look_now().unwrap();
            controller
        },
        source,
        store,
    };
    assert_eq!(
        f.planner_attempt(),
        1,
        "a wait for a model no longer configured ends on open"
    );
    f.finish_planning();
    assert_eq!(f.planner_models(), ["default-model", "alternate-model"]);
    let _ = &f.config;
}

fn put_quota(f: &mut Fixture, model: &str, reset: u64) {
    let snapshot = f.controller.snapshot().unwrap();
    let mut commit = Commit::new(
        format!("fixture-quota:{model}:{}", snapshot.revision.get()),
        json!({"fixture_quota": model}),
        &snapshot,
    );
    commit.rows.push(RowChange::Put {
        table: "model_quotas".into(),
        id: model.into(),
        value: json!({"reset_ms": reset}),
    });
    f.store.commit(&commit).unwrap();
}

// Stop before consuming the real worker's ending, so quota can be reported
// by another run while this successful reply is already in flight.
fn pending_success(settings: &tl3_config::Store) -> (Fixture, Observation) {
    let mut f = fixture(settings, &["default-model"], json!({}));
    let mut ending = None;
    wait_for_signal(|| {
        ending = f
            .controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .find_map(|row| {
                if row.table != "inbox" {
                    return None;
                }
                match serde_json::from_value::<Observation>(row.value.clone()).unwrap() {
                    observation @ Observation::WorkerReply { .. } => Some(observation),
                    _ => None,
                }
            });
        if ending.is_some() {
            return true;
        }
        f.controller.drive_once().unwrap();
        false
    });
    let ending = ending.unwrap();
    assert!(
        matches!(&ending, Observation::WorkerReply { model: Some(model), .. } if model == "default-model")
    );
    put_quota(&mut f, "default-model", real_now() + 600_000);
    put_quota(&mut f, "unrelated-model", real_now() + 600_000);
    (f, ending)
}

fn consume_success(f: &mut Fixture) {
    wait_for_signal(|| {
        if f.controller.world().unwrap().nodes[0].phase != Phase::Ingesting {
            return true;
        }
        f.controller.drive_once().unwrap();
        false
    });
}

pub(super) fn success_clears_quota(settings: &tl3_config::Store) {
    let (mut f, _) = pending_success(settings);
    consume_success(&mut f);
    assert_eq!(
        f.model_quota("default-model"),
        None,
        "a real reply ends the model's quota in its ending commit"
    );
    assert!(
        f.model_quota("unrelated-model").is_some(),
        "only the model that replied is cleared"
    );
}

pub(super) fn success_clears_quota_status(settings: &tl3_config::Store) {
    let (mut f, _) = pending_success(settings);
    consume_success(&mut f);
    let status =
        tl3_controller::status::load(&mut tl3_store::Reader::open(settings).unwrap()).unwrap();
    assert!(
        !status
            .quota_models
            .iter()
            .any(|line| line.contains("default-model"))
    );
    assert!(
        !tl3_controller::status::render_text(&status).contains("default-model is out of quota")
    );
    assert!(
        !tl3_controller::status::render_html(&status).contains("default-model is out of quota")
    );
}

pub(super) fn later_quota_is_recorded(settings: &tl3_config::Store) {
    let (mut f, _) = pending_success(settings);
    consume_success(&mut f);
    let reset = real_now() + 300_000;
    f.controller
        .observe(
            "later-quota",
            Observation::Quota {
                key: "later-worker".into(),
                reset_ms: reset,
                model: Some("default-model".into()),
            },
        )
        .unwrap();
    f.controller.drive_once().unwrap();
    assert_eq!(
        f.model_quota("default-model"),
        Some(reset),
        "a later quota ending creates a fresh record, even with an earlier announced reset"
    );
}

pub(super) fn replay_keeps_later_quota(settings: &tl3_config::Store) {
    let (mut f, ending) = pending_success(settings);
    consume_success(&mut f);
    let reset = real_now() + 900_000;
    put_quota(&mut f, "default-model", reset);
    f.controller
        .observe("success-replay", ending.clone())
        .unwrap();
    f.controller.drive_once().unwrap();
    assert_eq!(
        f.model_quota("default-model"),
        Some(reset),
        "a replayed success must not delete newer quota evidence"
    );
    let revision = f.controller.snapshot().unwrap().revision;
    f.controller.observe("success-replay", ending).unwrap();
    assert_eq!(
        f.controller.snapshot().unwrap().revision,
        revision,
        "the same ending is idempotent"
    );
}

/// #745: the worker's probe window for a model the proxy can't serve (`quota.PROBE_S`).
const PROBE_MS: u64 = 15 * 60 * 1000;

/// #745 (Q53): a model the proxy can't serve falls back like quota, and its
/// unavailability is recorded until a probe.
pub(super) fn unavailable_model_falls_back(settings: &tl3_config::Store, kind: &str) {
    let mut f = fixture(
        settings,
        &["default-model", "alternate-model"],
        json!({"planner": {"default-model": kind}}),
    );
    let before = real_now();
    assert!(
        f.planner_ended("default-model"),
        "{kind}: the unavailable model ends like quota, not as a failed run"
    );
    let probe = f
        .model_quota("default-model")
        .expect("the unavailability is recorded");
    assert!(
        (before + PROBE_MS..=real_now() + PROBE_MS).contains(&probe),
        "{kind}: the record expires at the probe, {probe}"
    );
    f.controller.look_due().unwrap();
    assert_eq!(f.planner_attempt(), 1, "the next model starts at once");
    f.finish_planning();
    assert_eq!(f.planner_models(), ["default-model", "alternate-model"]);
}

/// #745: once the record expires, the step tries the first entry again.
pub(super) fn unavailable_model_is_probed_after_expiry(settings: &tl3_config::Store) {
    let mut f = fixture(
        settings,
        &["default-model", "alternate-model"],
        json!({"planner": {"default-model": "model_not_found", "alternate-model": "retry_limit"}}),
    );
    assert!(
        f.planner_ended("default-model"),
        "the first model ends like quota"
    );
    f.controller.look_due().unwrap();
    assert_eq!(f.planner_attempt(), 1, "falls back to the second model");
    assert!(
        f.planner_ended("alternate-model"),
        "the second model ends like quota"
    );
    f.controller.look_due().unwrap();
    assert_eq!(
        f.planner_attempt(),
        1,
        "every model is out, so the step waits"
    );
    let probe = f.model_quota("default-model").unwrap();
    assert!(probe < f.model_quota("alternate-model").unwrap());
    // The proxy routes the first model again.
    fs::write(
        f.root.path().join("entered.quota-models"),
        json!({"planner": {"alternate-model": "retry_limit"}}).to_string(),
    )
    .unwrap();
    f.clock.0.set(probe - 1);
    f.controller.look_due().unwrap();
    assert_eq!(f.planner_attempt(), 1, "no probe before the record expires");
    f.clock.0.set(probe);
    f.controller.look_due().unwrap();
    assert_eq!(f.planner_attempt(), 2, "the expiry wakes the step");
    f.finish_planning();
    assert_eq!(
        f.planner_models(),
        ["default-model", "alternate-model", "default-model"],
        "the probe runs the first entry again"
    );
    assert_eq!(
        f.model_quota("default-model"),
        None,
        "the probed model's reply ends its record"
    );
}

/// #745: a 400 the request caused is a failed run, with no fallback.
pub(super) fn request_error_fails(settings: &tl3_config::Store) {
    let mut f = fixture(
        settings,
        &["default-model", "alternate-model"],
        json!({"planner": {"default-model": "bad_request"}}),
    );
    assert!(
        !f.planner_ended("default-model"),
        "a request-caused 400 fails the run"
    );
    assert_eq!(
        f.model_quota("default-model"),
        None,
        "and records no unavailability"
    );
}
