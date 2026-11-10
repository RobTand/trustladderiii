//! #783: the real CLI and store reader replay disposable fixture logs without writes.
#[path = "../../store/tests/support/mod.rs"]
mod support;
use serde_json::json;
use tl3_controller::{Bootstrap, Controller, LabelledIssue, Repository, adapters, replay};
use tl3_store::{Commit, Reader, RowChange, Store};

#[test]
fn replay_requires_initialized_unchanged_world_identity() {
    let mut db = support::Database::start();
    let settings = db.create("replay_identity");
    let root = tempfile::tempdir().unwrap();
    let mut controller = Controller::open(
        Store::open(&settings).unwrap(),
        adapters::simulation(root.path()).unwrap(),
        Bootstrap {
            operations: None,
            repos: vec![],
            rob: 42,
            slots: 1,
            pool: 2,
            fail_fast: false,
        },
    )
    .unwrap();
    let mut store = Store::open(&settings).unwrap();
    let snapshot = store.load().unwrap();
    let identity = snapshot
        .rows
        .iter()
        .find(|row| row.table == "kernel" && row.id == "world_id")
        .unwrap()
        .value
        .clone();
    let mut anchor = Commit::new("fixture:identity-anchor", json!({}), &snapshot);
    anchor.result = json!({"kernel_anchor":controller.world().unwrap()});
    store.commit(&anchor).unwrap();
    let mut reader = Reader::open(&settings).unwrap();
    replay::run(&mut reader, Some("fixture:identity-anchor")).unwrap();
    for (name, value, expected) in [
        ("rewritten", Some(identity.clone()), "changed after initialization"),
        ("changed", Some(json!(if identity == json!("0".repeat(32)) {
            "1".repeat(32)
        } else {
            "0".repeat(32)
        })), "changed after initialization"),
        ("malformed", Some(json!("invalid")), "stored world id must be 128 bits of lowercase hex"),
        ("missing", None, "stored world id is missing"),
    ] {
        let snapshot = store.load().unwrap();
        let mut commit = Commit::new(format!("fixture:identity-{name}"), json!({}), &snapshot);
        commit.rows.push(match value {
            Some(value) => RowChange::Put {
                table: "kernel".into(),
                id: "world_id".into(),
                value,
            },
            None => RowChange::Delete {
                table: "kernel".into(),
                id: "world_id".into(),
            },
        });
        store.commit(&commit).unwrap();
        let error = replay::run(&mut reader, Some("fixture:identity-anchor"))
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{name}: {error}");
    }
}

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
    // This branch never instruments apply. Existing history must fail explicitly.
    let legacy = replay::run(&mut Reader::open(&settings).unwrap(), None).unwrap_err();
    assert!(
        legacy
            .to_string()
            .contains("legacy history without recorded snapshots")
    );
    // Seed reviewed-format records directly into this disposable fixture only.
    let before = controller.world().unwrap();
    let event = tl3_kernel::Event::Fact {
        node: tl3_kernel::NodeId(0),
        fact: tl3_kernel::Fact::Priority(tl3_kernel::Priority::P0),
    };
    let after = tl3_kernel::step(before.clone(), event).world;
    let mut writer = Store::open(&settings).unwrap();
    let snapshot = writer.load().unwrap();
    let mut anchor = Commit::new("fixture:anchor", json!({}), &snapshot);
    anchor.result = json!({"kernel_anchor":before});
    writer.commit(&anchor).unwrap();
    let snapshot = writer.load().unwrap();
    let mut step = Commit::new("fixture:recorded-step", json!({}), &snapshot);
    step.result = json!({"kernel_replay":replay::ReplayRecord { before: before.clone(), steps: vec![replay::StepRecord {input:before,event,after:after.clone()}], after:after.clone() }});
    step.rows.push(RowChange::Put {
        table: "nodes".into(),
        id: "0".into(),
        value: json!(after.nodes[0]),
    });
    writer.commit(&step).unwrap();
    let original = controller.snapshot().unwrap();
    let mut reader = Reader::open(&settings).unwrap();
    let report = replay::run(&mut reader, Some("fixture:anchor")).unwrap();
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
        .args(["replay-check", "--from", "fixture:anchor", "--config"])
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
    for index in 0..130 {
        let snapshot = store.load().unwrap();
        store
            .commit(&Commit::new(
                format!("fixture:metadata:{index}"),
                json!({}),
                &snapshot,
            ))
            .unwrap();
    }
    assert!(
        replay::run(&mut reader, Some("fixture:anchor"))
            .unwrap()
            .events
            > 128
    );
    let after = store.load().unwrap();
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
    let snapshot = store.load().unwrap();
    store
        .commit(&Commit::new("fixture:later-metadata", json!({}), &snapshot))
        .unwrap();
    let error = replay::run(&mut reader, Some("fixture:anchor"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("fixture:corrupt-snapshot"), "{error}");
    assert!(error.contains("snapshot diverged"), "{error}");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_tl3"))
        .args(["replay-check", "--from", "fixture:anchor", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("fixture:corrupt-snapshot"));
}

/// #783: counts only, matching replay baselines, and SQL-enforced read-only access.
#[test]
fn store_stats_exact_counts_no_values_and_read_only() {
    let mut db = support::Database::start();
    let settings = db.create("store_stats_fixture");
    let mut store = Store::open(&settings).unwrap();
    let s = store.load().unwrap();
    let secret = "planted-secret-like-DO-NOT-PRINT-783";
    let mut first = Commit::new("fixture:legacy", json!({"secret":secret}), &s);
    first.rows.push(RowChange::Put {
        table: "requests".into(),
        id: "secret-row".into(),
        value: json!({"secret":secret}),
    });
    first.rows.push(RowChange::Put {
        table: "requests".into(),
        id: "another".into(),
        value: json!(secret),
    });
    store.commit(&first).unwrap();
    let s = store.load().unwrap();
    let mut baseline = Commit::new("fixture:baseline", json!({}), &s);
    let world = tl3_kernel::World::new(&[1], 1);
    baseline.result = json!({"kernel_replay":{"before":world,"steps":[],"after":world}});
    baseline.rows.push(RowChange::Put {
        table: "metadata".into(),
        id: "hidden".into(),
        value: json!(secret),
    });
    store.commit(&baseline).unwrap();
    let mut reader = Reader::open(&settings).unwrap();
    let stats = tl3_controller::store_stats::load(&mut reader).unwrap();
    assert_eq!(
        serde_json::to_value(&stats).unwrap(),
        json!({
            "world_rows":{"metadata":1,"requests":2},"events":2,
            "min_event_revision":1,"max_event_revision":2,"min_world_version":1,"replay_from_revision":2
        })
    );
    let json = serde_json::to_string(&stats).unwrap();
    assert!(!json.contains(secret));
    assert!(!json.contains("secret-row"));
    let text = tl3_controller::store_stats::render_text(&stats);
    assert!(!text.contains(secret));
    assert!(!text.contains("secret-row"));
    assert!(text.starts_with("metric\ttable\tcount\tmin_revision\tmax_revision\n"));
    let root = tempfile::tempdir().unwrap();
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
    let config = root.path().join("stats.toml");
    std::fs::write(&config, source).unwrap();
    let before = store.load().unwrap();
    for as_json in [false, true] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_tl3"));
        command.args(["store-stats", "--config"]).arg(&config);
        if as_json {
            command.arg("--json");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(!stdout.contains(secret));
        assert!(!stdout.contains("secret-row"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
        if as_json {
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&stdout).unwrap(),
                serde_json::to_value(&stats).unwrap()
            );
        } else {
            assert_eq!(stdout, text);
        }
    }
    let after = store.load().unwrap();
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.rows, after.rows);
    let mut tx = reader.transaction().unwrap();
    assert_eq!(tl3_controller::store_stats::read(&mut tx).unwrap(), stats);
    assert_eq!(
        tx.query_one("SHOW transaction_isolation", &[])
            .unwrap()
            .get::<_, String>(0),
        "repeatable read"
    );
    assert_eq!(
        tx.execute("DELETE FROM tl3_world", &[]).unwrap_err().code(),
        Some(&postgres::error::SqlState::READ_ONLY_SQL_TRANSACTION)
    );
}

/// #896 F0: explicit export is allowlisted, grouped, and SQL-enforced read-only.
#[test]
fn scope_trims_export_exact_records_summary_and_read_only() {
    let mut db = support::Database::start();
    let settings = db.create("scope_trims_fixture");
    let mut store = Store::open(&settings).unwrap();
    let secret = "planted-secret-DO-NOT-PRINT-896";
    let mut seed = Commit::new(
        "fixture:trims",
        json!({"credential":secret}),
        &store.load().unwrap(),
    );
    for (node, value) in [
        ("0", json!({"head":"old","removed_paths":[]})),
        (
            "1",
            json!({"head":"aaa","removed_paths":["tests/cache%20file","a%FF"]}),
        ),
        (
            "2",
            json!({"head":"bbb","removed_paths":["generated/output"]}),
        ),
        ("3", json!({"head":"ccc","removed_paths":[]})),
        (
            "4",
            json!({"head":secret,"removed_paths":[secret],"credential":secret}),
        ),
        ("5", json!({"head":secret})),
        ("6", json!({"head":"bad","removed_paths":secret})),
    ] {
        seed.rows.push(RowChange::Put {
            table: "scope_trims".into(),
            id: node.into(),
            value,
        });
    }
    for (node, owner) in [("0", "owner"), ("1", "owner"), ("2", "other")] {
        seed.rows.push(RowChange::Put {
            table: "nodes".into(),
            id: node.into(),
            value: json!({"repo":0,"secret":secret}),
        });
        seed.rows.push(RowChange::Put { table:"requests".into(), id:node.into(), value:json!({"issue":42,"repo":{"owner":owner,"name":"sandbox","installation":1,"credential":secret},"title":secret}) });
    }
    seed.rows.push(RowChange::Put {
        table: "unrelated".into(),
        id: "secret".into(),
        value: json!(secret),
    });
    store.commit(&seed).unwrap();
    let mut update = Commit::new("fixture:latest-trim", json!({}), &store.load().unwrap());
    update.rows.push(RowChange::Put {
        table: "scope_trims".into(),
        id: "0".into(),
        value: json!({"head":"ddd","removed_paths":[]}),
    });
    store.commit(&update).unwrap();
    let later = Commit::new("fixture:later", json!({}), &store.load().unwrap());
    store.commit(&later).unwrap();
    let before = store.load().unwrap();
    let mut reader = Reader::open(&settings).unwrap();
    let times: Vec<String> = reader.transaction().unwrap().query(
        "SELECT to_char(committed_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') FROM tl3_events ORDER BY revision", &[]
    ).unwrap().iter().map(|row| row.get(0)).collect();
    let expected = json!({
        "rows":[
            {"node":"0","request":42,"repo":"owner/sandbox","head":"ddd","removed_paths":[],"written_at":{"revision":2,"committed_at":times[1]}},
            {"node":"1","request":42,"repo":"owner/sandbox","head":"aaa","removed_paths":["tests/cache%20file","a%FF"],"written_at":{"revision":1,"committed_at":times[0]}},
            {"node":"2","request":42,"repo":"other/sandbox","head":"bbb","removed_paths":["generated/output"],"written_at":{"revision":1,"committed_at":times[0]}},
            {"node":"3","request":null,"repo":null,"head":"ccc","removed_paths":[],"written_at":{"revision":1,"committed_at":times[0]}},
            {"node":"4","invalid":true},{"node":"5","invalid":true},{"node":"6","invalid":true}
        ],
        "summary":{
            "total_rows":7,"non_empty_rows":2,
            "by_repo":[
                {"repo":null,"total_rows":4,"non_empty_rows":0},
                {"repo":"other/sandbox","total_rows":1,"non_empty_rows":1},
                {"repo":"owner/sandbox","total_rows":2,"non_empty_rows":1}
            ],
            "by_request":[
                {"repo":null,"request":null,"total_rows":4,"non_empty_rows":0},
                {"repo":"other/sandbox","request":42,"total_rows":1,"non_empty_rows":1},
                {"repo":"owner/sandbox","request":42,"total_rows":2,"non_empty_rows":1}
            ]
        }
    });
    let root = tempfile::tempdir().unwrap();
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
    let config = root.path().join("stats.toml");
    std::fs::write(&config, source).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_tl3"))
        .args(["store-stats", "--scope-trims", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        expected
    );
    let stats = tl3_controller::store_stats::load(&mut reader).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_tl3"))
        .args(["store-stats", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "metric\ttable\tcount\tmin_revision\tmax_revision\nworld_rows\tnodes\t3\t-\t-\nworld_rows\trequests\t3\t-\t-\nworld_rows\tscope_trims\t7\t-\t-\nworld_rows\tunrelated\t1\t-\t-\nevents\t-\t3\t1\t3\nworld_version\t-\t-\t1\t-\nreplay_from\t-\t-\t-\t-\n"
    );
    assert!(!serde_json::to_string(&stats).unwrap().contains(secret));
    let after = store.load().unwrap();
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.rows, after.rows);
    let mut tx = reader.transaction().unwrap();
    assert_eq!(
        tl3_controller::store_stats::read_scope_trims(&mut tx).unwrap(),
        expected
    );
    assert_eq!(
        tx.query_one("SHOW transaction_isolation", &[])
            .unwrap()
            .get::<_, String>(0),
        "repeatable read"
    );
    assert_eq!(
        tx.query_one("SHOW transaction_read_only", &[])
            .unwrap()
            .get::<_, String>(0),
        "on"
    );
    assert_eq!(
        tx.execute("DELETE FROM tl3_world", &[]).unwrap_err().code(),
        Some(&postgres::error::SqlState::READ_ONLY_SQL_TRANSACTION)
    );
}

/// #896 review SF1/SF2: damaged event history must not hide current trim rows.
#[test]
fn scope_trims_missing_event_preserves_row_and_total() {
    let mut db = support::Database::start();
    let settings = db.create("scope_trims_missing_event");
    let mut store = Store::open(&settings).unwrap();
    let mut seed = Commit::new("fixture:trims", json!({}), &store.load().unwrap());
    for (node, paths) in [("0", vec![]), ("1", vec!["cache%20file"])] {
        seed.rows.push(RowChange::Put {
            table: "scope_trims".into(),
            id: node.into(),
            value: json!({"head":"aaa","removed_paths":paths}),
        });
    }
    store.commit(&seed).unwrap();
    let mut sql = postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap();
    // Only this disposable fixture bypasses the FK to model damaged history.
    // The setting resets on commit; the export still uses the normal Reader.
    let mut tx = sql.transaction().unwrap();
    tx.batch_execute("SET LOCAL session_replication_role = replica")
        .unwrap();
    tx.execute(
        "UPDATE tl3_world SET version = 999 WHERE table_name = 'scope_trims' AND row_id = '1'",
        &[],
    )
    .unwrap();
    tx.commit().unwrap();
    let mut reader = Reader::open(&settings).unwrap();
    let stats = tl3_controller::store_stats::load(&mut reader).unwrap();
    let export = tl3_controller::store_stats::load_scope_trims(&mut reader).unwrap();
    assert_eq!(
        export["summary"]["total_rows"],
        stats.world_rows["scope_trims"]
    );
    assert_eq!(export["rows"].as_array().unwrap().len(), 2);
    assert_eq!(
        export["rows"][1],
        json!({
            "node":"1","request":null,"repo":null,"head":"aaa",
            "removed_paths":["cache%20file"],
            "written_at":{"revision":999,"committed_at":null}
        })
    );
    assert_eq!(
        export["summary"],
        json!({
            "total_rows":2,"non_empty_rows":1,
            "by_repo":[{"repo":null,"total_rows":2,"non_empty_rows":1}],
            "by_request":[{"repo":null,"request":null,"total_rows":2,"non_empty_rows":1}]
        })
    );
}
