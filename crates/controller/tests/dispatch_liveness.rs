#[path = "../../store/tests/support/mod.rs"]
mod support;

use std::{cell::RefCell, path::Path, rc::Rc};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    executor::{Completion, Delivery, Executor, Observation, Output, Reconciliation, Upstream},
    status,
};
use tl3_kernel::{EffectKind, Event, NodeId, Phase, Reply, Role};
use tl3_store::{Commit, Reader, RowChange, Store};

#[derive(Default)]
struct Runs {
    starts: Vec<String>,
    failed: Option<String>,
}

struct Workers {
    fake: adapters::workers::Fake,
    hold: Role,
    runs: Rc<RefCell<Runs>>,
}

impl Executor for Workers {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        if matches!(delivery.output, Output::Effect { kind: EffectKind::StartRun(role), .. } if role == self.hold)
        {
            self.runs.borrow_mut().starts.push(delivery.key.clone());
            Ok(Completion::confirmed(vec![]))
        } else {
            self.fake.execute(delivery)
        }
    }

    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        if matches!(delivery.output, Output::Effect { kind: EffectKind::StartRun(role), .. } if role == self.hold)
        {
            if self.runs.borrow().failed.as_ref() == Some(&delivery.key) {
                return Ok(Reconciliation::Found(Box::new(Completion::confirmed(
                    vec![Observation::WorkerFailed {
                        node: delivery.node(),
                        generation: delivery.generation,
                        failure_class: "OpenAIRateLimitError".into(),
                        failure_message: "worker run failed".into(),
                    }],
                ))));
            }
            Ok(Reconciliation::Pending)
        } else {
            self.fake.reconcile(delivery)
        }
    }
}

fn open(
    settings: &tl3_config::Store,
    root: &Path,
    role: Role,
    runs: &Rc<RefCell<Runs>>,
) -> Controller {
    let mut adapters = adapters::simulation(root).unwrap();
    adapters.register(
        Upstream::Workers,
        Workers {
            fake: adapters::workers::Fake::new(root),
            hold: role,
            runs: runs.clone(),
        },
    );
    Controller::open(
        Store::open(settings).unwrap(),
        adapters,
        Bootstrap {
            repos: vec![Repository {
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
    )
    .unwrap()
}

fn intake(controller: &mut Controller, number: u64) {
    controller
        .intake(LabelledIssue {
            delivery: format!("label-{number}"),
            owner: "owner".into(),
            repo: "repo".into(),
            installation: 1,
            number,
            title: "Keep worker waits live".into(),
            actor: 42,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    drain(controller);
}

fn drain(controller: &mut Controller) {
    for _ in 0..300 {
        if !controller.drive_once().unwrap() {
            return;
        }
    }
    panic!("controller did not settle");
}

fn lose_watch(settings: &tl3_config::Store, key: &str) {
    let mut store = Store::open(settings).unwrap();
    let snapshot = store.load().unwrap();
    let mut commit = Commit::new("fault:lost-watch", serde_json::json!({}), &snapshot);
    for table in ["watches", "allowances"] {
        commit.rows.push(RowChange::Delete {
            table: table.into(),
            id: key.into(),
        });
    }
    store.commit(&commit).unwrap();
}

fn raw(settings: &tl3_config::Store) -> postgres::Client {
    postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap()
}

fn close(controller: Controller, settings: &tl3_config::Store) {
    drop(controller);
    let mut sql = raw(settings);
    sql.batch_execute("SET lock_timeout = '180s'").unwrap();
    sql.query_one("SELECT pg_advisory_lock($1)", &[&0x544c33434f4e5452_i64])
        .unwrap();
    sql.query_one("SELECT pg_advisory_unlock($1)", &[&0x544c33434f4e5452_i64])
        .unwrap();
}

#[test]
fn failed_audit_without_a_watch_recovers_on_startup() {
    let mut db = support::Database::start();
    let settings = db.create("lost_audit");
    let root = tempfile::tempdir().unwrap();
    let runs = Rc::new(RefCell::new(Runs::default()));
    let mut controller = open(&settings, root.path(), Role::Audit, &runs);
    intake(&mut controller, 2662);
    let before = controller.world().unwrap().nodes[0];
    assert_eq!(before.phase, Phase::Auditing);
    let key = runs.borrow().starts[0].clone();
    runs.borrow_mut().failed = Some(key.clone());
    lose_watch(&settings, &key);
    close(controller, &settings);
    let mut controller = open(&settings, root.path(), Role::Audit, &runs);
    drain(&mut controller);
    let after = controller.world().unwrap().nodes[0];
    assert!(
        after.generation > before.generation,
        "failed audit was never reconciled"
    );
    assert_eq!(after.run_failures, 1);
    assert_eq!(after.credit, before.credit);
    assert_eq!(runs.borrow().starts.len(), 2, "one infrastructure retry");
}

#[test]
fn startup_reattaches_every_worker_phase_without_duplicate_launches() {
    let mut db = support::Database::start();
    for (index, role) in [
        Role::Ingest,
        Role::Audit,
        Role::Plan,
        Role::Make,
        Role::Review,
    ]
    .into_iter()
    .enumerate()
    {
        for advanced_epoch in [false, true] {
            let settings = db.create(&format!("phase_{index}_{advanced_epoch}"));
            let root = tempfile::tempdir().unwrap();
            let runs = Rc::new(RefCell::new(Runs::default()));
            let mut controller = open(&settings, root.path(), role, &runs);
            intake(&mut controller, 1);
            let before = controller.world().unwrap();
            let key = runs.borrow().starts[0].clone();
            lose_watch(&settings, &key);
            if advanced_epoch {
                let mut store = Store::open(&settings).unwrap();
                let snapshot = store.load().unwrap();
                let mut commit = Commit::new("ownership-advance", serde_json::json!({}), &snapshot);
                commit.next_epoch = Some(tl3_store::Epoch::try_from(1).unwrap());
                store.commit(&commit).unwrap();
            }
            close(controller, &settings);
            let mut controller = open(&settings, root.path(), role, &runs);
            drain(&mut controller);
            let snapshot = controller.snapshot().unwrap();
            assert!(
                snapshot
                    .rows
                    .iter()
                    .any(|r| r.table == "watches" && r.id == key),
                "{role:?} has no durable result watch after restart"
            );
            assert!(
                snapshot
                    .rows
                    .iter()
                    .any(|r| r.table == "allowances" && r.id == key)
            );
            assert_eq!(controller.world().unwrap(), before);
            assert!(
                !snapshot
                    .rows
                    .iter()
                    .any(|r| r.table == "incidents" && r.value["open"] == true)
            );
            let watch = snapshot
                .rows
                .iter()
                .find(|r| r.table == "watches" && r.id == key)
                .unwrap();
            assert_eq!(watch.value["epoch"], serde_json::json!(0));
            assert_eq!(runs.borrow().starts.len(), 1);
            let revision = snapshot.revision;
            drain(&mut controller);
            assert_eq!(controller.snapshot().unwrap().revision, revision);
        }
    }
}

#[test]
fn pool_change_repairs_an_orphan_without_a_clock_tick() {
    let mut db = support::Database::start();
    let settings = db.create("pool_event");
    let root = tempfile::tempdir().unwrap();
    let runs = Rc::new(RefCell::new(Runs::default()));
    let mut controller = open(&settings, root.path(), Role::Audit, &runs);
    intake(&mut controller, 1);
    intake(&mut controller, 2);
    let key = runs.borrow().starts[0].clone();
    lose_watch(&settings, &key);
    let generation = controller.world().unwrap().nodes[1].generation;
    controller
        .observe(
            "other-audit-finished",
            Observation::Kernel(Event::Reply {
                node: NodeId(1),
                generation,
                reply: Reply::AuditAgrees,
            }),
        )
        .unwrap();
    drain(&mut controller);
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "watches" && r.id == key),
        "pool changes leave the orphan silently waiting"
    );
    assert_eq!(
        runs.borrow().starts.iter().filter(|k| **k == key).count(),
        1
    );
}

#[test]
fn stale_dispatch_raises_one_visible_incident_instead_of_relaunching() {
    let mut db = support::Database::start();
    let settings = db.create("stale_dispatch");
    let root = tempfile::tempdir().unwrap();
    let runs = Rc::new(RefCell::new(Runs::default()));
    let mut controller = open(&settings, root.path(), Role::Audit, &runs);
    intake(&mut controller, 1);
    let key = runs.borrow().starts[0].clone();
    lose_watch(&settings, &key);
    // Model an imported wait whose only durable dispatch is an older generation.
    let mut store = Store::open(&settings).unwrap();
    let snapshot = store.load().unwrap();
    let mut value = snapshot
        .rows
        .iter()
        .find(|r| r.table == "nodes" && r.id == "0")
        .unwrap()
        .value
        .clone();
    value["generation"] = serde_json::json!(999);
    value["run"] = serde_json::json!(999);
    let mut commit = Commit::new("fault:stale-dispatch", serde_json::json!({}), &snapshot);
    commit.rows.push(RowChange::Put {
        table: "nodes".into(),
        id: "0".into(),
        value,
    });
    store.commit(&commit).unwrap();
    let before = controller.world().unwrap();
    close(controller, &settings);
    let mut controller = open(&settings, root.path(), Role::Audit, &runs);
    drain(&mut controller);
    let visible = status::load(&mut Reader::open(&settings).unwrap()).unwrap();
    assert_eq!(
        visible.requests[0].details.open_incidents.len(),
        1,
        "orphan wait is silent"
    );
    assert_eq!(controller.world().unwrap(), before);
    assert_eq!(runs.borrow().starts.len(), 1);
    close(controller, &settings);
    let mut controller = open(&settings, root.path(), Role::Audit, &runs);
    drain(&mut controller);
    assert_eq!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.table == "incidents" && r.value["open"] == true)
            .count(),
        1,
        "startup must deduplicate the incident"
    );
}

fn set_next_seq(controller: &mut Controller, settings: &tl3_config::Store, next_seq: u64) {
    let snapshot = controller.snapshot().unwrap();
    let row = snapshot
        .rows
        .iter()
        .find(|r| r.table == "nodes" && r.id == "0")
        .unwrap();
    let mut value = row.value.clone();
    value["next_seq"] = serde_json::json!(next_seq);
    let mut commit = Commit::new(
        format!("counter:{next_seq}"),
        serde_json::json!({}),
        &snapshot,
    );
    commit.rows.push(RowChange::Put {
        table: "nodes".into(),
        id: "0".into(),
        value,
    });
    Store::open(settings).unwrap().commit(&commit).unwrap();
}

fn missing_incidents(controller: &mut Controller) -> usize {
    controller
        .snapshot()
        .unwrap()
        .rows
        .iter()
        .filter(|r| {
            r.table == "incidents" && r.value["signature"]["class"] == "worker_dispatch_missing"
        })
        .count()
}

#[test]
fn startup_defers_dispatch_repair_while_counter_headroom_is_exhausted() {
    let mut db = support::Database::start();
    let settings = db.create("exhausted_repair");
    let root = tempfile::tempdir().unwrap();
    let runs = Rc::new(RefCell::new(Runs::default()));
    let mut controller = open(&settings, root.path(), Role::Audit, &runs);
    intake(&mut controller, 1);
    let next_seq = controller.world().unwrap().nodes[0].next_seq;
    let key = runs.borrow().starts[0].clone();
    lose_watch(&settings, &key);
    set_next_seq(&mut controller, &settings, u64::MAX);
    close(controller, &settings);
    let mut controller = open(&settings, root.path(), Role::Audit, &runs);
    assert_eq!(
        missing_incidents(&mut controller),
        0,
        "counter exhaustion is not a missing dispatch"
    );
    drain(&mut controller);
    assert_eq!(missing_incidents(&mut controller), 0);
    set_next_seq(&mut controller, &settings, next_seq);
    close(controller, &settings);
    let mut controller = open(&settings, root.path(), Role::Audit, &runs);
    drain(&mut controller);
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "watches" && r.id == key)
    );
    assert_eq!(runs.borrow().starts.len(), 1);
}

#[test]
fn startup_preserves_retained_input_and_its_counter_pause() {
    let mut db = support::Database::start();
    let settings = db.create("retained_repair");
    let root = tempfile::tempdir().unwrap();
    let runs = Rc::new(RefCell::new(Runs::default()));
    let mut controller = open(&settings, root.path(), Role::Audit, &runs);
    intake(&mut controller, 1);
    let key = runs.borrow().starts[0].clone();
    lose_watch(&settings, &key);
    set_next_seq(&mut controller, &settings, u64::MAX);
    controller
        .observe(
            "retained-priority",
            Observation::Kernel(Event::Fact {
                node: NodeId(0),
                fact: tl3_kernel::Fact::Priority(tl3_kernel::Priority::P1),
            }),
        )
        .unwrap();
    close(controller, &settings);
    let mut controller = open(&settings, root.path(), Role::Audit, &runs);
    assert_eq!(
        missing_incidents(&mut controller),
        0,
        "startup must leave retained input to its headroom guard"
    );
    drain(&mut controller);
    let snapshot = controller.snapshot().unwrap();
    assert!(
        snapshot
            .rows
            .iter()
            .any(|r| r.table == "inbox" && r.id == "external:retained-priority")
    );
    assert_eq!(
        snapshot
            .rows
            .iter()
            .filter(|r| r.table == "incidents"
                && r.value["signature"]["class"] == "kernel_counter_headroom"
                && r.value["open"] == true)
            .count(),
        1
    );
    assert_eq!(missing_incidents(&mut controller), 0);
    let revision = controller.snapshot().unwrap().revision;
    assert!(!controller.drive_once().unwrap());
    assert_eq!(controller.snapshot().unwrap().revision, revision);
}

#[test]
fn pending_start_stays_on_its_original_dispatch_path() {
    let mut db = support::Database::start();
    let settings = db.create("pending_start_repair");
    let root = tempfile::tempdir().unwrap();
    let runs = Rc::new(RefCell::new(Runs::default()));
    let mut controller = open(&settings, root.path(), Role::Ingest, &runs);
    controller
        .intake(LabelledIssue {
            delivery: "pending-label".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            installation: 1,
            number: 1,
            title: "Keep pending starts on the dispatch path".into(),
            actor: 42,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    let node = controller.world().unwrap().nodes[0];
    let key = format!("effect:0:{}", node.effect.unwrap().seq);
    assert_eq!(runs.borrow().starts.len(), 0);
    lose_watch(&settings, &key);
    close(controller, &settings);
    let mut controller = open(&settings, root.path(), Role::Ingest, &runs);
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "watches" && r.id == key),
        "a pending start must settle through the original effect, before repairing its acknowledged result watch"
    );
    assert_eq!(missing_incidents(&mut controller), 0);
    drain(&mut controller);
    assert_eq!(runs.borrow().starts.as_slice(), std::slice::from_ref(&key));
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "watches" && r.id == key)
    );
}

#[test]
fn unrelated_observations_and_idle_drives_visit_no_worker_waits() {
    let mut db = support::Database::start();
    let settings = db.create("no_per_drive_scan");
    let root = tempfile::tempdir().unwrap();
    let runs = Rc::new(RefCell::new(Runs::default()));
    let mut controller = open(&settings, root.path(), Role::Audit, &runs);
    intake(&mut controller, 1);
    let key = runs.borrow().starts[0].clone();
    lose_watch(&settings, &key);
    controller
        .observe(
            "priority-only",
            Observation::Kernel(Event::Fact {
                node: NodeId(0),
                fact: tl3_kernel::Fact::Priority(tl3_kernel::Priority::P1),
            }),
        )
        .unwrap();
    let mut visited = 0;
    for _ in 0..20 {
        let progressed = controller
            .drive_once_with(|boundary, _| {
                if boundary == tl3_controller::Boundary::WorkerWait {
                    visited += 1;
                }
            })
            .unwrap();
        if !progressed {
            break;
        }
    }
    assert_eq!(
        controller.world().unwrap().nodes[0].priority,
        tl3_kernel::Priority::P1
    );
    assert_eq!(
        visited, 0,
        "unrelated input and idle drives must do no worker-wait reconciliation"
    );
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "watches" && r.id == key)
    );
}

/// #816 P2 / counter headroom: unchanged drives and allowance wakes do no rescan.
#[test]
fn exhausted_recovery_is_parked_between_clearance_events() {
    let mut db = support::Database::start();
    let settings = db.create("t817_parked_repair");
    let root = tempfile::tempdir().unwrap();
    let runs = Rc::new(RefCell::new(Runs::default()));
    let mut controller = open(&settings, root.path(), Role::Audit, &runs);
    intake(&mut controller, 1);
    let key = runs.borrow().starts[0].clone();
    lose_watch(&settings, &key);
    set_next_seq(&mut controller, &settings, u64::MAX);
    close(controller, &settings);
    let mut controller = open(&settings, root.path(), Role::Audit, &runs);
    let mut scans = 0;
    for _ in 0..12 {
        controller
            .drive_once_with(|boundary, _| {
                if boundary == tl3_controller::Boundary::WorkerWaitHeadroom {
                    scans += 1;
                }
            })
            .unwrap();
    }
    assert_eq!(
        scans, 0,
        "parked exhaustion recovery scanned on unrelated drives"
    );
}

/// Q39 and state-table "Pausing": neither wait implies a dispatched operation.
fn t817_assert_undispatched_wait(
    controller: &mut Controller,
    settings: &tl3_config::Store,
    expected: tl3_kernel::Wait,
) {
    let classified = controller.dispatch_status(NodeId(2)).unwrap();
    assert_eq!(
        classified,
        if expected == tl3_kernel::Wait::AgentPool {
            tl3_controller::dispatch_liveness::DispatchStatus::PoolWait
        } else {
            tl3_controller::dispatch_liveness::DispatchStatus::Paused
        }
    );
    let world = controller.world().unwrap();
    assert!(world.valid());
    assert!(world.consistent());
    let node = world.nodes[2];
    assert_eq!(node.phase, Phase::Ingesting);
    assert_eq!(node.waiting_on(), expected);
    assert_eq!(
        node.ticket.is_some(),
        expected == tl3_kernel::Wait::AgentPool
    );
    assert!(!node.pool);
    assert!(node.effect.is_none() && node.run.is_none() && node.job.is_none());
    assert_eq!(
        node.next_seq, 0,
        "this node has never dispatched any effect"
    );
    assert_eq!((node.run_failures, node.fx_failures), (0, 0));
    assert!(!node.waiting_incident);
    let snapshot = controller.snapshot().unwrap();
    assert!(
        snapshot
            .pending_effects
            .iter()
            .all(|e| !e.key.starts_with("effect:2:"))
    );
    assert!(
        Reader::open(settings)
            .unwrap()
            .effects()
            .unwrap()
            .iter()
            .all(|e| !e.key.starts_with("effect:2:"))
    );
    for row in &snapshot.rows {
        if row.table == "watches" {
            let delivery: Delivery = serde_json::from_value(row.value.clone()).unwrap();
            assert_ne!(delivery.node(), NodeId(2), "case (a) has no watch");
        }
        if row.table == "allowances" {
            let delivery: Delivery = serde_json::from_value(row.value["delivery"].clone()).unwrap();
            assert_ne!(delivery.node(), NodeId(2), "case (c) has no deadline");
        }
        assert!(
            row.table != "incidents" || row.value["open"] != true,
            "case (d) has no open incident"
        );
    }
    assert_eq!(
        node.generation, 0,
        "no operation has started for this request"
    );
    println!(
        "T817_COUNTEREXAMPLE wait={expected:?} node=2 phase=Ingesting next_seq=0 run=None job=None failures=0 watches=0 allowances=0 open_incidents=0"
    );
}

/// Q39 permits waiting behind occupied grants, including accepted starvation.
#[test]
fn t817_pool_wait_classifies_without_an_incident() {
    let mut db = support::Database::start();
    let settings = db.create("t817_pool_scope");
    let root = tempfile::tempdir().unwrap();
    let runs = Rc::new(RefCell::new(Runs::default()));
    let mut controller = open(&settings, root.path(), Role::Ingest, &runs);
    for number in 1..=3 {
        intake(&mut controller, number);
    }
    assert_eq!(runs.borrow().starts.len(), 2);
    for _ in 0..3 {
        t817_assert_undispatched_wait(&mut controller, &settings, tl3_kernel::Wait::AgentPool);
        let before = controller.world().unwrap();
        for _ in 0..4 {
            assert!(!controller.drive_once().unwrap());
            assert_eq!(controller.world().unwrap(), before);
        }
        close(controller, &settings);
        controller = open(&settings, root.path(), Role::Ingest, &runs);
    }
    t817_assert_undispatched_wait(&mut controller, &settings, tl3_kernel::Wait::AgentPool);
    assert_eq!(runs.borrow().starts.len(), 2);
}

/// State-table "Pausing" keeps the pending start until an authorized Resume.
#[test]
fn t817_pause_classifies_without_an_incident() {
    let mut db = support::Database::start();
    let settings = db.create("t817_resume_scope");
    let root = tempfile::tempdir().unwrap();
    let runs = Rc::new(RefCell::new(Runs::default()));
    let mut controller = open(&settings, root.path(), Role::Ingest, &runs);
    for number in 1..=3 {
        intake(&mut controller, number);
    }
    tl3_store::ConsoleCommands::open(&settings)
        .unwrap()
        .submit(
            "00000000-0000-4000-8000-000000000817",
            2,
            &serde_json::json!({"kind":"pause"}),
        )
        .unwrap();
    tl3_controller::ingress::drain(&mut controller).unwrap();
    drain(&mut controller);
    for _ in 0..3 {
        t817_assert_undispatched_wait(&mut controller, &settings, tl3_kernel::Wait::Resume);
        let before = controller.world().unwrap();
        for _ in 0..4 {
            assert!(!controller.drive_once().unwrap());
            assert_eq!(controller.world().unwrap(), before);
        }
        close(controller, &settings);
        controller = open(&settings, root.path(), Role::Ingest, &runs);
    }
    t817_assert_undispatched_wait(&mut controller, &settings, tl3_kernel::Wait::Resume);
    assert_eq!(runs.borrow().starts.len(), 2);
}
