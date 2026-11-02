//! #371: an upgrade replays observations an older release already committed.
#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::{Value, json};
use tl3_controller::{Bootstrap, Controller, Repository, adapters, executor::Observation};
use tl3_kernel::NodeId;
use tl3_store::{Commit, Store};

fn bootstrap() -> Bootstrap {
    Bootstrap {
        repos: vec![Repository {
            owner: "owner".into(),
            name: "sandbox".into(),
            installation: 3,
        }],
        rob: 42,
        slots: 1,
        pool: 1,
        operations: None,
    }
}

/// What the older release stored under `observe:<key>`.
fn store_old_event(settings: &tl3_config::Store, key: &str, payload: Value) {
    let mut store = Store::open(settings).unwrap();
    let snapshot = store.load().unwrap();
    store
        .commit(&Commit::new(format!("observe:{key}"), payload, &snapshot))
        .unwrap();
}

fn reply(generation: u64, model: Option<&str>) -> Observation {
    Observation::WorkerReply {
        node: NodeId(0),
        generation,
        role: "maker".into(),
        reply: json!({"kind": "done", "head": "abc"}),
        model: model.map(str::to_owned),
    }
}

#[test]
fn upgrade_replays_keep_the_stored_receipt_and_conflicts_name_the_key() {
    let mut database = support::Database::start();
    let settings = database.create("upgrade_replay");
    let root = tempfile::tempdir().unwrap();
    // Shapes the release before #351 committed, without the model field.
    store_old_event(
        &settings,
        "effect:0:7:worker:0:0",
        json!({"WorkerReply": {"node": 0, "generation": 3, "role": "maker",
            "reply": {"kind": "done", "head": "abc"}}}),
    );
    store_old_event(
        &settings,
        "effect:0:5:worker:1:0",
        json!({"Quota": {"key": "effect:0:5", "reset_ms": 1_793_774_125_705_u64}}),
    );
    // The shape 43f6506d committed when a model was unknown.
    store_old_event(
        &settings,
        "effect:0:9:worker:0:0",
        json!({"WorkerReply": {"node": 0, "generation": 4, "role": "maker",
            "reply": {"kind": "done", "head": "abc"}, "model": null}}),
    );
    let mut controller = Controller::open(
        Store::open(&settings).unwrap(),
        adapters::simulation(root.path()).unwrap(),
        bootstrap(),
    )
    .unwrap();
    let before = controller.snapshot().unwrap();
    // The worker source re-reads finished runs at start-up, now naming the model.
    controller
        .observe("effect:0:7:worker:0:0", reply(3, Some("claude-opus-5-5")))
        .expect("a newer shape of the same reply keeps the stored receipt");
    controller
        .observe(
            "effect:0:5:worker:1:0",
            Observation::Quota {
                key: "effect:0:5".into(),
                reset_ms: 1_793_774_125_705,
                model: Some("gpt-6.1-sol".into()),
            },
        )
        .expect("a newer shape of the same quota ending keeps the stored receipt");
    controller
        .observe("effect:0:9:worker:0:0", reply(4, None))
        .expect("an absent model matches a stored null");
    let after = controller.snapshot().unwrap();
    assert_eq!(
        after.revision, before.revision,
        "replays commit nothing new"
    );
    assert!(!after.rows.iter().any(|r| r.table == "inbox"));
    // A different fact under the same key still stops, and the error names the key.
    let error = controller
        .observe("effect:0:7:worker:0:0", reply(8, Some("claude-opus-5-5")))
        .unwrap_err();
    assert!(
        error.to_string().contains("observe:effect:0:7:worker:0:0"),
        "{error}"
    );
    assert_eq!(controller.snapshot().unwrap().revision, before.revision);
    database.stop();
}
