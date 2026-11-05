//! #817 / Q7: real-store schedules, real inotify/pidfd inspection, fixed seeds.
//! Only worker execution is replaced: the recorded live identity is this test
//! process. No subprocess, wall-clock advance or timeout establishes failure.
use super::*;
use crate::dispatch_liveness::{
    DispatchStatus, Evidence, ProcessEvidence, classify, decision::DeliveryClass, model,
};
use crate::executor::Upstream;
use crate::{Bootstrap, Controller, LabelledIssue, adapters};
use proptest::prelude::*;
use std::{cell::RefCell, rc::Rc};
use tl3_kernel::{NodeId, Role, Wait};
use tl3_store::{Commit, Reader, RowChange};

struct PendingRecords {
    root: PathBuf,
    starts: Rc<RefCell<BTreeMap<String, usize>>>,
    fake: Fake,
}

struct RecordedClock;
impl crate::clock::Clock for RecordedClock {
    fn now(&self) -> std::time::SystemTime {
        // Request 235's quota ending was recorded on 2026-11-03. The reset
        // remains days away regardless of when the regression executes.
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_793_712_000_000)
    }
}
fn identity() -> Value {
    let stat = fs::read_to_string("/proc/self/stat").unwrap();
    let start = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse::<u64>()
        .unwrap();
    json!({"pid":std::process::id(),"start":start,
        "machine":fs::read_to_string("/etc/machine-id").unwrap().trim(),
        "boot":fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap().trim()})
}
impl Executor for PendingRecords {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        if !matches!(
            delivery.output,
            Out::Effect {
                kind: EffectKind::StartRun(Role::Ingest),
                ..
            }
        ) {
            return self.fake.execute(delivery);
        }
        let mut starts = self.starts.borrow_mut();
        let count = starts.entry(delivery.key.clone()).or_default();
        *count += 1;
        assert_eq!(*count, 1, "recovery launched a second start");
        let attempt = run_root(&self.root, &delivery.key).join("attempt-00000000");
        fs::create_dir_all(&attempt)?;
        for name in ["claim.json", "process.json"] {
            process::write(&attempt.join(name), &identity())?;
        }
        // A complete-looking stdout without exit.json must never become a reply.
        fs::write(attempt.join("stdout.jsonl"), b"{\"event\":\"reply\",\"role\":\"ingest\",\"reply\":{\"kind\":\"early_end\",\"outcome\":\"not_actionable\",\"reason\":\"uncommitted\"}}\n")?;
        Ok(Completion::confirmed(vec![]))
    }
    fn reconcile(&mut self, _: &Delivery) -> Result<Reconciliation> {
        // The #815 case (2): the supervisor API stays Pending forever.
        Ok(Reconciliation::Pending)
    }
    fn dispatch_evidence(&mut self, delivery: &Delivery) -> Result<(ProcessEvidence, bool)> {
        let attempt = run_root(&self.root, &delivery.key).join("attempt-00000000");
        Ok((
            if !attempt.join("claim.json").exists() && !attempt.join("process.json").exists() {
                ProcessEvidence::Unknown
            } else if alive(&attempt.join("claim.json"))? || alive(&attempt.join("process.json"))? {
                ProcessEvidence::Live
            } else {
                ProcessEvidence::Ended
            },
            false,
        ))
    }
}

fn open(
    settings: &tl3_config::Store,
    root: &Path,
    starts: &Rc<RefCell<BTreeMap<String, usize>>>,
) -> Controller {
    let mut registry = adapters::simulation(&root.join("fakes")).unwrap();
    registry.register(
        Upstream::Workers,
        PendingRecords {
            root: root.join("runs"),
            starts: starts.clone(),
            fake: Fake::new(&root.join("fakes")),
        },
    );
    Controller::open_with_clock(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap {
            repos: vec![crate::Repository {
                owner: "owner".into(),
                name: "repo".into(),
                installation: 1,
            }],
            operations: None,
            rob: 42,
            slots: 2,
            pool: 2,
            fail_fast: true,
        },
        RecordedClock,
    )
    .unwrap()
}
fn settle(controller: &mut Controller) {
    for _ in 0..32 {
        if !controller.drive_once().unwrap() {
            return;
        }
    }
    panic!("exceeded controller settle bound");
}
fn close(controller: Controller, settings: &tl3_config::Store) {
    drop(controller);
    // Ownership's lock holder releases asynchronously; join it with the same
    // advisory lock through the disposable store, with no sleep or process scan.
    let mut client = postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap();
    client
        .query_one("SELECT pg_advisory_lock($1)", &[&0x544c33434f4e5452_i64])
        .unwrap();
    client
        .query_one("SELECT pg_advisory_unlock($1)", &[&0x544c33434f4e5452_i64])
        .unwrap();
}
fn evidence() -> Evidence {
    Evidence {
        class: DeliveryClass::Worker,
        intent: true,
        contract: crate::dispatch_liveness::decision::ContractEvidence::Absent,
        watch: true,
        process: ProcessEvidence::Live,
        ending: false,
        deadline: true,
        quota_reset: None,
        incident: false,
        paused: false,
        ticket: false,
        obstruction: false,
        question: false,
        external: false,
        closed: false,
        admission: false,
    }
}

// The reference state is a projection of raw durable facts, not the public
// classifier. The same proved recovery model predicts the post-restart state;
// ingress and each drive are checked separately, before another step can hide it.
fn sampled(controller: &mut Controller, root: &Path) -> Evidence {
    let node = controller.world().unwrap().nodes[0];
    let snapshot = controller.snapshot().unwrap();
    let mut e = evidence();
    e.watch = false;
    e.deadline = false;
    e.process = ProcessEvidence::Unknown;
    e.intent = false;
    e.incident = snapshot
        .rows
        .iter()
        .any(|r| r.table == "incidents" && r.value["open"] == true && r.value["node"] == 0);
    e.paused = node.paused;
    e.ticket = node.ticket.is_some();
    let Some(seq) = node
        .effect
        .map(|e| e.seq)
        .or_else(|| node.next_seq.checked_sub(1))
    else {
        return e;
    };
    let key = format!("effect:0:{seq}");
    e.intent = controller
        .store
        .effect(&key)
        .unwrap()
        .is_some_and(|effect| effect.payload["generation"] == json!(node.generation));
    for row in &snapshot.rows {
        if row.id == key {
            if row.table == "watches" {
                e.watch = row.value["generation"] == json!(node.generation);
            }
            if row.table == "allowances"
                && row.value["delivery"]["generation"] == json!(node.generation)
            {
                e.deadline = row.value["due_ms"].as_u64().is_some();
                e.quota_reset = row.value["quota_ms"].as_u64();
            }
        }
        if row.table == "inbox" {
            let failure = &row.value["WorkerFailed"];
            e.ending |= failure["node"] == 0 && failure["generation"] == json!(node.generation);
        }
    }
    let attempt = run_root(&root.join("runs"), &key).join("attempt-00000000");
    if attempt.join("claim.json").exists() || attempt.join("process.json").exists() {
        e.process = if alive(&attempt.join("claim.json")).unwrap()
            || alive(&attempt.join("process.json")).unwrap()
        {
            ProcessEvidence::Live
        } else {
            ProcessEvidence::Ended
        };
    }
    e
}

fn assert_step(
    controller: &mut Controller,
    root: &Path,
    starts: &Rc<RefCell<BTreeMap<String, usize>>>,
) {
    let e = sampled(controller, root);
    let node = controller.world().unwrap().nodes[0];
    assert_eq!(
        controller.dispatch_status(NodeId(0)).unwrap(),
        classify(node.waiting_on(), e),
        "per-step reference state: {e:?}"
    );
    assert!(starts.borrow().values().all(|n| *n == 1), "second start");
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .all(|r| r.table != "briefs"),
        "invented reply"
    );
}

fn restart_boundary(
    mut controller: Controller,
    settings: &tl3_config::Store,
    root: &Path,
    starts: &Rc<RefCell<BTreeMap<String, usize>>>,
    restart: bool,
) -> Controller {
    assert_step(&mut controller, root, starts);
    if !restart {
        return controller;
    }
    let before = controller.world().unwrap();
    let snapshot = controller.snapshot().unwrap();
    let original = sampled(&mut controller, root);
    let node = before.nodes[0];
    let acknowledged = node.effect.is_none() && node.run.is_some() && !node.waiting_incident;
    let repairable = acknowledged
        && !crate::input_errors::kernel_backlog(&snapshot)
        && !original.incident
        && !original.ending
        && original.quota_reset.is_none();
    // A startup repair does not import an ending. That requires the separate
    // source/ingress boundary. Model its transaction with unknown liveness.
    let mut expected = original;
    if repairable {
        let mut recovery = original;
        recovery.process = ProcessEvidence::Unknown;
        expected = model::step(recovery, original.intent);
        expected.process = original.process;
    }
    let effects = Reader::open(settings).unwrap().effects().unwrap();
    close(controller, settings);
    let mut controller = open(settings, root, starts);
    assert_eq!(
        controller.world().unwrap(),
        before,
        "restart changed kernel state"
    );
    assert_eq!(
        sampled(&mut controller, root),
        expected,
        "restart recovery differs from reference transaction"
    );
    let effect_facts = |effects: Vec<tl3_store::Effect>| {
        effects
            .into_iter()
            .map(|e| {
                (
                    e.key,
                    e.epoch.get(),
                    e.kind,
                    e.payload,
                    e.state,
                    e.created_revision.get(),
                    e.updated_revision.get(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        effect_facts(Reader::open(settings).unwrap().effects().unwrap()),
        effect_facts(effects),
        "restart changed immutable dispatches"
    );
    assert_step(&mut controller, root, starts);
    controller
}

fn schedule(
    actions: &[u8],
    restarts: &[bool],
    index: usize,
    database: &mut crate::git_tests::support::Database,
) {
    let settings = database.create(&format!("dispatch_model_{index}"));
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("runs/.ready")).unwrap();
    let starts = Rc::new(RefCell::new(BTreeMap::new()));
    let mut controller = open(&settings, root.path(), &starts);
    controller
        .intake(LabelledIssue {
            delivery: "start".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            installation: 1,
            number: 1,
            title: "Recover dispatch evidence".into(),
            actor: 42,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    settle(&mut controller);
    let mut failures = 0;
    let mut boundary = 0;
    macro_rules! checkpoint {
        () => {{
            let restart = restarts[boundary % restarts.len()];
            boundary += 1;
            controller = restart_boundary(controller, &settings, root.path(), &starts, restart);
            restart
        }};
    }
    for (step, action) in actions.iter().enumerate() {
        let node = controller.world().unwrap().nodes[0];
        // The missing-dispatch incident is authoritative controller evidence;
        // it need not set the kernel's waiting_incident flag. Incidents absorb
        // subsequent schedule actions in the reference model.
        if node.waiting_incident
            || controller.dispatch_status(NodeId(0)).unwrap() == DispatchStatus::Incident
        {
            break;
        }
        let key = format!("effect:0:{}", node.next_seq - 1);
        let mut expected = evidence();
        let mut store = Store::open(&settings).unwrap();
        let snapshot = store.load().unwrap();
        let mut fault = Commit::new(format!("fault:{step}"), json!({"fault":action}), &snapshot);
        let tables: &[&str] = match action {
            1 => {
                expected.watch = false;
                &["watches"]
            }
            2 => {
                expected.deadline = false;
                &["allowances"]
            }
            3 => {
                expected.watch = false;
                expected.deadline = false;
                &["watches", "allowances"]
            }
            _ => &[],
        };
        for table in tables {
            fault.rows.push(RowChange::Delete {
                table: (*table).into(),
                id: key.clone(),
            });
        }
        store.commit(&fault).unwrap();
        if *action == 3 {
            let mut sql = postgres::Config::new()
                .host(&settings.host)
                .port(settings.port)
                .user(&settings.user)
                .dbname(&settings.database)
                .password("tl3-disposable-test-password")
                .connect(postgres::NoTls)
                .unwrap();
            sql.execute("DELETE FROM tl3_effects WHERE effect_key=$1", &[&key])
                .unwrap();
        }
        if *action == 4 {
            expected.process = ProcessEvidence::Ended;
            let attempt = run_root(&root.path().join("runs"), &key).join("attempt-00000000");
            let mut dead = identity();
            dead["pid"] = json!(2147483647);
            dead["start"] = json!(1);
            for name in ["claim.json", "process.json"] {
                process::write(&attempt.join(name), &dead).unwrap();
            }
        }
        expected = model::step(expected, *action != 3);
        let wanted = classify(Wait::WorkerReply, expected);
        // Faults are recovered once, then arbitrary additional process restarts
        // interrupt every source/ingress/drive boundary in the schedule.
        controller = restart_boundary(controller, &settings, root.path(), &starts, true);
        checkpoint!();
        let mut source = source::WorkerOutputSource::new(
            root.path().join("runs"),
            settings.clone(),
            ActiveRuns::default(),
        );
        let (sender, mut receiver) = crate::sync::mpsc::channel();
        source.poll_once(&mut store, &sender).unwrap();
        if checkpoint!() {
            // A process restart loses its channel. Recreate both source and
            // channel, then replay solely from the worker's durable evidence.
            source = source::WorkerOutputSource::new(
                root.path().join("runs"),
                settings.clone(),
                ActiveRuns::default(),
            );
            let (replay_sender, replay_receiver) = crate::sync::mpsc::channel();
            source.poll_once(&mut store, &replay_sender).unwrap();
            receiver = replay_receiver;
            assert_step(&mut controller, root.path(), &starts);
        }
        while let Ok(input) = receiver.try_recv() {
            if let crate::cli::Input::Observation { key, observation } = input {
                if checkpoint!() {
                    // The ingress payload must be recoverable after losing the
                    // old channel, including at the pre-observe crash point.
                    let mut replay = source::WorkerOutputSource::new(
                        root.path().join("runs"),
                        settings.clone(),
                        ActiveRuns::default(),
                    );
                    let (tx, rx) = crate::sync::mpsc::channel();
                    replay.poll_once(&mut store, &tx).unwrap();
                    assert!(rx.try_iter().any(|input| matches!(input,
                        crate::cli::Input::Observation { key: recovered, observation: value }
                        if recovered == key && serde_json::to_value(&value).unwrap() == serde_json::to_value(&observation).unwrap())),
                        "ingress was not replayable from durable evidence");
                }
                controller.observe(&key, observation).unwrap();
                checkpoint!();
            }
        }
        // K=32 individual drives, with a generated crash choice before and
        // after EACH drive. Never hide the drain behind a single restart.
        let mut settled = false;
        for _ in 0..32 {
            checkpoint!();
            let progressed = controller.drive_once().unwrap();
            checkpoint!();
            if !progressed {
                settled = true;
                break;
            }
        }
        assert!(settled, "exceeded K=32 drives: {actions:?}, {restarts:?}");
        let actual = controller.dispatch_status(NodeId(0)).unwrap();
        let after = controller.world().unwrap().nodes[0];
        match wanted {
            DispatchStatus::Ending => {
                failures += 1;
                assert_eq!(after.run_failures, failures % 2);
                assert_eq!(after.run_incidents, failures / 2);
                assert!(after.generation > node.generation || after.waiting_incident);
                let failure = controller
                    .snapshot()
                    .unwrap()
                    .rows
                    .into_iter()
                    .find(|r| r.table == "worker_failures" && r.id == "0")
                    .unwrap();
                assert_eq!(failure.value["generation"], json!(node.generation));
                assert_eq!(failure.value["failure_class"], "LostWorker");
                assert_eq!(
                    after.credit, node.credit,
                    "infrastructure loss spends no credit"
                );
                assert_eq!(
                    after.waiting_incident,
                    failures == 2,
                    "Q7's second failure opens an incident"
                );
            }
            _ => assert_eq!(actual, wanted, "schedule {actions:?} step {step}"),
        }
        assert!(matches!(
            actual,
            DispatchStatus::Dispatched(_)
                | DispatchStatus::Ending
                | DispatchStatus::Retry(_)
                | DispatchStatus::Incident
        ));
        assert!(starts.borrow().values().all(|n| *n == 1));
        assert!(
            controller
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .all(|r| r.table != "briefs"),
            "invented reply"
        );
        // Recovery never allocates another immutable start for the same generation.
        let effects = Reader::open(&settings).unwrap().effects().unwrap();
        for generation in 0..=after.generation {
            assert!(
                effects
                    .iter()
                    .filter(|e| e.payload["generation"] == json!(generation)
                        && e.payload["output"]["Effect"]["kind"]["StartRun"] == "Ingest")
                    .count()
                    <= 1
            );
        }
        drop(source);
    }
    assert!(boundary > 0);
    close(controller, &settings);
}

/// #817 / Q7: fixed witnesses plus generated real-store fault/restart schedules.
#[test]
fn reference_model_regressions_conform_to_store_faults_and_restarts() {
    let mut database = crate::git_tests::support::Database::start();
    let regressions: Vec<Vec<u8>> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/dispatch-conformance-regressions.json"
    ))
    .unwrap();
    for (i, actions) in regressions.iter().enumerate() {
        schedule(actions, &[true], i, &mut database);
    }
    #[derive(Deserialize)]
    struct RestartRegression {
        actions: Vec<u8>,
        restarts: Vec<bool>,
    }
    let interrupted: Vec<RestartRegression> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/dispatch-restart-regressions.json"
    ))
    .unwrap();
    for (i, case) in interrupted.iter().enumerate() {
        schedule(
            &case.actions,
            &case.restarts,
            regressions.len() + i,
            &mut database,
        );
    }
}

fn generated_schedules(seed: u64) {
    use proptest::test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner};
    let index = std::cell::Cell::new(0);
    let database = RefCell::new(crate::git_tests::support::Database::start());
    // Bazel's source tree is read-only. Persist into its published artifact
    // directory, seeding it with the committed regressions on every run.
    let fallback = tempfile::tempdir().unwrap();
    let output = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback.path().to_owned());
    fs::create_dir_all(&output).unwrap();
    let persistence = output.join(format!("dispatch-conformance-{seed}.proptest-regressions"));
    fs::write(
        &persistence,
        include_str!("../../../tests/fixtures/dispatch-conformance.proptest-regressions"),
    )
    .unwrap();
    let persistence = Box::leak(persistence.to_string_lossy().into_owned().into_boxed_str());
    let mut runner = TestRunner::new(Config {
        cases: 16,
        max_shrink_iters: 128,
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(persistence))),
        ..Config::default()
    });
    runner
        .run(
            &(
                prop::collection::vec(0u8..6, 1..5),
                prop::collection::vec(any::<bool>(), 128),
            ),
            |(actions, restarts)| {
                schedule(&actions, &restarts, index.get(), &mut database.borrow_mut());
                index.set(index.get() + 1);
                Ok(())
            },
        )
        .unwrap();
    eprintln!(
        "T817_CONFORMANCE seed={seed} cases={} choices=128 drives=32",
        index.get()
    );
}

/// #817 / Q7: independent generated schedules keep each conformance unit bounded.
#[test]
fn reference_model_seed_817815_conforms_to_store_faults_and_restarts() {
    generated_schedules(817_815);
}

/// #817 / Q7: independent generated schedules keep each conformance unit bounded.
#[test]
fn reference_model_seed_817816_conforms_to_store_faults_and_restarts() {
    generated_schedules(817_816);
}

/// #817 / Q7: independent generated schedules keep each conformance unit bounded.
#[test]
fn reference_model_seed_817817_conforms_to_store_faults_and_restarts() {
    generated_schedules(817_817);
}

/// #817 / Q7: independent generated schedules keep each conformance unit bounded.
#[test]
fn reference_model_seed_817818_conforms_to_store_faults_and_restarts() {
    generated_schedules(817_818);
}

/// Q53 / #828: request 235's multi-day quota reset is a valid retry deadline.
#[test]
fn quota_ending_is_a_model_quota_retry_across_recovery() {
    use crate::dispatch_liveness::decision::RetryPending;
    let mut database = crate::git_tests::support::Database::start();
    let settings = database.create("dispatch_quota");
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("runs/.ready")).unwrap();
    let starts = Rc::new(RefCell::new(BTreeMap::new()));
    let mut controller = open(&settings, root.path(), &starts);
    controller
        .intake(LabelledIssue {
            delivery: "quota".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            installation: 1,
            number: 1,
            title: "Recorded quota".into(),
            actor: 42,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    settle(&mut controller);
    let node = controller.world().unwrap().nodes[0];
    let key = format!("effect:0:{}", node.next_seq - 1);
    let until = 1_794_088_801_362;
    controller
        .observe(
            "quota-ending",
            Observation::Quota {
                key: key.clone(),
                reset_ms: until,
                model: Some("claude-sonnet-5-5".into()),
            },
        )
        .unwrap();
    settle(&mut controller);
    let attempt = run_root(&root.path().join("runs"), &key).join("attempt-00000000");
    let mut dead = identity();
    dead["pid"] = json!(2147483647);
    dead["start"] = json!(1);
    for name in ["claim.json", "process.json"] {
        process::write(&attempt.join(name), &dead).unwrap();
    }
    let mut store = Store::open(&settings).unwrap();
    let mut source = source::WorkerOutputSource::new(
        root.path().join("runs"),
        settings.clone(),
        ActiveRuns::default(),
    );
    let (sender, receiver) = crate::sync::mpsc::channel();
    source.poll_once(&mut store, &sender).unwrap();
    assert!(
        receiver.try_recv().is_err(),
        "a durable quota ending is not process loss"
    );
    let snapshot = store.load().unwrap();
    let mut fault = Commit::new("quota-lost-watch", json!({}), &snapshot);
    fault.rows.push(RowChange::Delete {
        table: "watches".into(),
        id: key.clone(),
    });
    store.commit(&fault).unwrap();
    let before = controller.snapshot().unwrap();
    close(controller, &settings);
    let mut controller = open(&settings, root.path(), &starts);
    for _ in 0..4 {
        assert_eq!(
            controller.dispatch_status(NodeId(0)).unwrap(),
            DispatchStatus::Retry(RetryPending::ModelQuota { until })
        );
        controller.drive_once().unwrap();
    }
    let after = controller.snapshot().unwrap();
    let relevant = |snapshot: &tl3_store::Snapshot| {
        snapshot
            .rows
            .iter()
            .filter(|r| {
                matches!(
                    r.table.as_str(),
                    "allowances" | "watches" | "incidents" | "worker_failures"
                )
            })
            .map(|r| (r.table.clone(), r.id.clone(), r.value.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(relevant(&before), relevant(&after));
    assert_eq!(controller.world().unwrap().nodes[0], node);
    assert_eq!(starts.borrow().values().sum::<usize>(), 1);
    close(controller, &settings);
}
