//! #783: the real CLI and store reader replay disposable fixture logs without writes.
#[path = "../../store/tests/support/mod.rs"]
mod support;
use serde_json::json;
use tl3_controller::{Bootstrap, Controller, LabelledIssue, Repository, adapters, replay};
use tl3_store::{Commit, Reader, RowChange, Store};

/// #783 / Q27: SQL-enforced read-only access, inclusive restart, corrupt current rows.
#[test]
fn fixture_log_read_only_cli_and_corrupt_snapshot() {
    let mut db = support::Database::start();
    let settings = db.create("replay_fixture");
    let root = tempfile::tempdir().unwrap();
    let bootstrap = Bootstrap {
        operations: None,
        repos: vec![Repository {
            owner: "owner".into(),
            name: "sandbox".into(),
            installation: 1,
        }],
        rob: 42,
        slots: 1,
        pool: 2,
        fail_fast: false,
    };
    let mut controller = Controller::open(
        Store::open(&settings).unwrap(),
        adapters::simulation(root.path()).unwrap(),
        bootstrap,
    )
    .unwrap();
    controller
        .intake(LabelledIssue {
            delivery: "fixture:intake".into(),
            owner: "owner".into(),
            repo: "sandbox".into(),
            installation: 1,
            number: 42,
            title: "Replay fixture".into(),
            actor: 42,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    let original = controller.snapshot().unwrap();
    let mut reader = Reader::open(&settings).unwrap();
    let report = replay::run(&mut reader, None).unwrap();
    assert!(report.events > 0);
    assert_eq!(report.steps, 1);
    let events = reader.events_after(tl3_store::Revision::ZERO, 128).unwrap();
    let step = events
        .iter()
        .find(|e| e.receipt.result.get("kernel_replay").is_some())
        .unwrap();
    let from_key = replay::run(&mut reader, Some(&step.key)).unwrap();
    let from_revision =
        replay::run(&mut reader, Some(&step.receipt.revision.get().to_string())).unwrap();
    assert_eq!(from_key.steps, 1);
    assert_eq!(from_revision.steps, 1);
    assert!(replay::run(&mut reader, Some("nonexistent")).is_err());
    {
        let mut tx = reader.transaction().unwrap();
        assert_eq!(
            tx.query_one("SHOW transaction_read_only", &[])
                .unwrap()
                .get::<_, String>(0),
            "on"
        );
        let error = tx
            .execute(
                "UPDATE tl3_meta SET revision = revision WHERE singleton",
                &[],
            )
            .unwrap_err();
        assert_eq!(
            error.code(),
            Some(&postgres::error::SqlState::READ_ONLY_SQL_TRANSACTION)
        );
    }
    let source = include_str!("../../config/example.toml")
        .replace("port = 5432", &format!("port = {}", settings.port))
        .replace(
            "database = \"tl3\"",
            &format!("database = {:?}", settings.database),
        )
        .replace("user = \"tl3\"", "user = \"postgres\"")
        .replace(
            "# password_file = \"/run/tl3/postgres-password\"",
            &format!(
                "password_file = {:?}",
                settings.password_file.as_ref().unwrap()
            ),
        );
    let config = root.path().join("replay.toml");
    std::fs::write(&config, source).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_tl3"))
        .args(["replay-check", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 kernel steps"));
    let after = controller.snapshot().unwrap();
    assert_eq!(after.revision, original.revision);
    assert_eq!(after.rows, original.rows);
    assert_eq!(
        reader.events_after(tl3_store::Revision::ZERO, 128).unwrap(),
        events
    );

    // A valid but wrong snapshot must fail equality even though its invariants hold.
    let mut store = Store::open(&settings).unwrap();
    let mut corrupt = Commit::new("fixture:corrupt-snapshot", json!({}), &after);
    let meta = after.rows.iter().find(|r| r.table == "kernel").unwrap();
    let mut value = meta.value.clone();
    value["world"]["pool_capacity"] = json!(3);
    corrupt.rows.push(RowChange::Put {
        table: "kernel".into(),
        id: "meta".into(),
        value,
    });
    store.commit(&corrupt).unwrap();
    let error = replay::run(&mut reader, None).unwrap_err().to_string();
    assert!(error.contains("fixture:corrupt-snapshot"), "{error}");
    assert!(error.contains("snapshot diverged"), "{error}");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_tl3"))
        .args(["replay-check", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("fixture:corrupt-snapshot"));
}
