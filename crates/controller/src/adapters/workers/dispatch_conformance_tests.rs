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
            if alive(&attempt.join("claim.json"))? || alive(&attempt.join("process.json"))? {
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

fn schedule(actions: &[u8], index: usize, database: &mut crate::git_tests::support::Database) {
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
        // Every schedule boundary restarts both controller and event source.
        close(controller, &settings);
        controller = open(&settings, root.path(), &starts);
        let mut source = source::WorkerOutputSource::new(
            root.path().join("runs"),
            settings.clone(),
            ActiveRuns::default(),
        );
        let (sender, receiver) = crate::sync::mpsc::channel();
        source.poll_once(&mut store, &sender).unwrap();
        while let Ok(input) = receiver.try_recv() {
            if let crate::cli::Input::Observation { key, observation } = input {
                controller.observe(&key, observation).unwrap();
            }
        }
        // K=32 controller drives covers the fixed one-node transaction backlog
        // of this fixture. No iteration advances time or probes the worker.
        settle(&mut controller);
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
        schedule(actions, i, &mut database);
    }
}

fn generated_schedules(seed: u64) {
    use proptest::test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner};
    let index = std::cell::Cell::new(0);
    let database = RefCell::new(crate::git_tests::support::Database::start());
    let mut runner = TestRunner::new(Config {
        cases: 2,
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/dispatch-conformance.proptest-regressions"
        )))),
        ..Config::default()
    });
    runner
        .run(&prop::collection::vec(0u8..6, 1..9), |actions| {
            schedule(&actions, index.get(), &mut database.borrow_mut());
            index.set(index.get() + 1);
            Ok(())
        })
        .unwrap();
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
