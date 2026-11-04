//! #371: an upgrade replays observations an older release already committed.
#[path = "support/ownership.rs"]
mod startup_ownership;
#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::{Value, json};
use std::{fs, path::Path, sync::mpsc};
use tl3_controller::{
    Bootstrap, Controller, Repository,
    adapters::{self, workers::Supervisor},
    cli::Input,
    executor::{Delivery, Observation},
    ingress,
};
use tl3_kernel::NodeId;
use tl3_store::{Commit, NewEffect, RowChange, Store};

const MODEL: &str = "claude-opus-5-5";

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
        fail_fast: false,
        operations: None,
    }
}

fn commit(settings: &tl3_config::Store, key: &str, payload: Value, rows: Vec<RowChange>) {
    let mut store = Store::open(settings).unwrap();
    let snapshot = store.load().unwrap();
    let mut change = Commit::new(key, payload, &snapshot);
    change.rows = rows;
    store.commit(&change).unwrap();
}

fn ingest_run() -> Delivery {
    serde_json::from_value(json!({
        "key": "effect:0:3", "epoch": 1,
        "output": {"Effect": {"node": 0, "seq": 3, "kind": {"StartRun": "Ingest"}, "head": null}},
        "generation": 2, "head_sha": null,
        "repository": {"owner": "owner", "name": "sandbox", "installation": 3},
        "issue": 12, "pull_request": null, "paths": [], "attempt": 1, "round": 0
    }))
    .unwrap()
}

/// A run the older release had finished and observed: its records stay on disk.
fn finished_attempt(runs: &Path, run: &Delivery, reply: &Value) {
    let root = runs.join(
        run.key
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    );
    let attempt = root.join("attempt-00000000");
    fs::create_dir_all(&attempt).unwrap();
    // This machine, an earlier boot: the process is gone.
    let claim = json!({"pid": 1, "machine": fs::read_to_string("/etc/machine-id").unwrap().trim(),
        "start": 0, "boot": "an-earlier-boot"});
    fs::write(attempt.join("claim.json"), claim.to_string()).unwrap();
    fs::write(attempt.join("exit.json"), json!({"code": 0}).to_string()).unwrap();
    fs::write(
        attempt.join("request.json"),
        json!({"role": "ingest", "model": {"name": MODEL}, "inputs": {}}).to_string(),
    )
    .unwrap();
    fs::write(
        attempt.join("stdout.jsonl"),
        format!(
            "{}\n",
            json!({"event": "reply", "role": "ingest", "reply": reply})
        ),
    )
    .unwrap();
}

#[test]
fn startup_rereads_a_finished_run_in_the_newer_shape_and_the_loop_survives() {
    let mut database = support::Database::start();
    let settings = database.create("upgrade_replay");
    let root = tempfile::tempdir().unwrap();
    let mut config = tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
    config.store = settings.clone();
    config.workers.runs = root.path().join("runs");
    config.hindsight.endpoint = "http://127.0.0.1:9".parse().unwrap();
    let run = ingest_run();
    let reply =
        json!({"kind": "early_end", "outcome": "duplicate", "reason": "Already requested."});
    let key = format!("{}:worker:0:0", run.key);
    // Opening initializes the world; the older release's records come after it.
    let mut controller = Controller::open(
        Store::open(&settings).unwrap(),
        adapters::simulation(root.path()).unwrap(),
        bootstrap(),
    )
    .unwrap();
    // The release before #351 observed this reply without a model, and still watches the run.
    commit(
        &settings,
        &format!("observe:{key}"),
        json!({"WorkerReply": {"node": 0, "generation": 2, "role": "ingest", "reply": reply}}),
        vec![],
    );
    {
        let mut store = Store::open(&settings).unwrap();
        let snapshot = store.load().unwrap();
        let mut change = Commit::new("watch-run", json!({"watch": run.key}), &snapshot);
        change.rows = vec![RowChange::Put {
            table: "watches".into(),
            id: run.key.clone(),
            value: json!(run),
        }];
        change.effects = vec![NewEffect {
            key: run.key.clone(),
            kind: "Workers".into(),
            payload: json!(run),
        }];
        store.commit(&change).unwrap();
    }
    finished_attempt(&config.workers.runs, &run, &reply);
    let before = controller.snapshot().unwrap().revision;
    // Start-up: the new release's worker source rescans watched runs.
    let supervisor = Supervisor::new(
        &config,
        &root.path().join("records"),
        &crate::startup_ownership::ownership(),
    )
    .unwrap();
    let mut source = supervisor.source();
    let (sender, receiver) = mpsc::channel();
    source
        .poll_once(&mut Store::open(&settings).unwrap(), &sender)
        .unwrap();
    let inputs: Vec<Input> = receiver.try_iter().collect();
    let replay = inputs
        .into_iter()
        .find(|input| matches!(input, Input::Observation { key: k, .. } if *k == key))
        .expect("the source re-reads the finished run under the stored key");
    let Input::Observation { observation, .. } = &replay else {
        unreachable!()
    };
    assert!(
        matches!(observation, Observation::WorkerReply { model: Some(m), .. } if m == MODEL),
        "the replay names the model the stored event lacks"
    );
    ingress::apply(&mut controller, replay).expect("the main loop survives the replay");
    let after = controller.snapshot().unwrap();
    assert_eq!(after.revision, before, "the stored receipt stands");
    assert!(!after.rows.iter().any(|r| r.table == "inbox"));
    database.stop();
}

#[test]
fn replays_of_other_shapes_keep_the_receipt_and_conflicts_name_the_key() {
    let mut database = support::Database::start();
    let settings = database.create("upgrade_shapes");
    let root = tempfile::tempdir().unwrap();
    // The release before #351 stored quota endings without a model.
    commit(
        &settings,
        "observe:effect:0:5:worker:1:0",
        json!({"Quota": {"key": "effect:0:5", "reset_ms": 1_793_774_125_705_u64}}),
        vec![],
    );
    // 43f6506d stored an absent model as null.
    commit(
        &settings,
        "observe:effect:0:9:worker:0:0",
        json!({"WorkerReply": {"node": 0, "generation": 4, "role": "maker",
            "reply": {"kind": "done"}, "model": null}}),
        vec![],
    );
    let mut controller = Controller::open(
        Store::open(&settings).unwrap(),
        adapters::simulation(root.path()).unwrap(),
        bootstrap(),
    )
    .unwrap();
    let before = controller.snapshot().unwrap().revision;
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
    let reply = |generation| Observation::WorkerReply {
        node: NodeId(0),
        generation,
        role: "maker".into(),
        reply: json!({"kind": "done"}),
        model: None,
    };
    controller
        .observe("effect:0:9:worker:0:0", reply(4))
        .expect("an absent model matches a stored null");
    assert_eq!(controller.snapshot().unwrap().revision, before);
    let error = controller
        .observe("effect:0:9:worker:0:0", reply(8))
        .unwrap_err();
    assert!(
        error.to_string().contains("observe:effect:0:9:worker:0:0"),
        "{error}"
    );
    assert_eq!(controller.snapshot().unwrap().revision, before);
    database.stop();
}
