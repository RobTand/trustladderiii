//! #731: the library's tests share one Postgres per process, never a database.
use crate::store_support::Database;
use serde_json::json;
use tl3_store::Store;

#[test]
fn tests_share_one_server_but_never_a_database() {
    let mut first = Database::start();
    let mut second = Database::start();
    let a = first.create("shared_isolation");
    let b = second.create("shared_isolation");
    assert_eq!(
        (a.host.as_str(), a.port),
        (b.host.as_str(), b.port),
        "one server per test process"
    );
    assert_ne!(
        a.database, b.database,
        "a reused name still gets its own database"
    );
    let mut store = Store::open(&a).unwrap();
    store
        .put_worker_materials("0", &json!({"owner": "first"}))
        .unwrap();
    assert_eq!(
        Store::open(&b).unwrap().worker_materials("0").unwrap(),
        None,
        "one test's rows never reach another's database"
    );
    drop(store);
    first.stop();
    assert!(Store::open(&a).is_err(), "stop ends the test's databases");
    assert!(
        Store::open(&b).is_ok(),
        "and leaves other tests' databases alone"
    );
}

#[test]
fn footprint_interns_aliases_once_and_reports_invalid_planner_identity() {
    use crate::{Bootstrap, Controller, LabelledIssue, Repository, adapters};
    use tl3_kernel::NodeId;

    let mut db = Database::start();
    let settings = db.create("footprint_intake");
    let root = tempfile::tempdir().unwrap();
    let mut controller = Controller::open(
        Store::open(&settings).unwrap(),
        adapters::simulation(root.path()).unwrap(),
        Bootstrap {
            repos: vec![Repository {
                owner: "owner".into(),
                name: "sandbox".into(),
                installation: 3,
            }],
            rob: 42,
            slots: 1,
            pool: 1,
            fail_fast: false,
            operations: None,
        },
    )
    .unwrap();
    controller
        .intake(LabelledIssue {
            delivery: "footprint".into(),
            owner: "owner".into(),
            repo: "sandbox".into(),
            installation: 3,
            number: 3312,
            title: "Delete the declared file".into(),
            actor: 42,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    let snapshot = controller.snapshot().unwrap();
    let mut world = controller.world().unwrap();
    world.nodes[0].run = Some(17);
    world.nodes[0].attempt = 2;
    let canonical = controller
        .footprint(
            &snapshot,
            &mut world,
            NodeId(0),
            &["x.py".into(), "a/b.py".into()],
        )
        .unwrap();
    let aliases = controller
        .footprint(
            &snapshot,
            &mut world,
            NodeId(0),
            &[
                "./x.py".into(),
                "x.py/".into(),
                "a//b.py".into(),
                "a/./b.py".into(),
            ],
        )
        .unwrap();
    assert_eq!(aliases, canonical);
    for path in ["../escape", "a/.git/b", "a\\b", "bad\n.py"] {
        let before = controller.snapshot().unwrap().interned;
        let error = controller
            .footprint(
                &snapshot,
                &mut world,
                NodeId(0),
                &["not-interned.py".into(), path.into()],
            )
            .unwrap_err();
        let diagnostic = error.to_string();
        for expected in [
            "invariant",
            "owner/sandbox#3312",
            "node 0",
            "attempt 2",
            "run 17",
        ] {
            assert!(
                diagnostic.contains(expected),
                "{diagnostic:?} lacks {expected}"
            );
        }
        assert!(diagnostic.contains(&format!("{path:?}")), "{diagnostic}");
        assert_eq!(
            controller.snapshot().unwrap().interned,
            before,
            "invalid intake must not intern a valid prefix"
        );
    }
}
