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
