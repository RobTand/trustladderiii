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
        .find(|e| e.receipt.result.get("kernel_delta").is_some())
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
    assert!(replay::run(&mut reader, None).unwrap().events > 128);
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

/// #783: kill the real store writer between event/delta and world-row writes.
/// Atomic commit must recover to a verifiable prefix, including after commit.
#[test]
fn compact_commit_crash_safety() {
    use std::{
        io::{BufRead, Write},
        process::{Command, Stdio},
    };
    use tl3_controller::compact::{CompactStep, Delta, Record};
    if let Some(path) = std::env::var_os("TL3_COMPACT_CRASH_CONFIG") {
        let config = tl3_config::Config::load_status(path).unwrap();
        let data: serde_json::Value = serde_json::from_slice(
            &std::fs::read(std::env::var_os("TL3_COMPACT_CRASH_DATA").unwrap()).unwrap(),
        )
        .unwrap();
        let before: tl3_kernel::World = serde_json::from_value(data["before"].clone()).unwrap();
        let event = tl3_kernel::Event::Fact {
            node: tl3_kernel::NodeId(0),
            fact: tl3_kernel::Fact::Priority(tl3_kernel::Priority::P0),
        };
        let after = tl3_kernel::step(before.clone(), event).0;
        let mut store = Store::open(&config.store).unwrap();
        let s = store.load().unwrap();
        let mut commit = Commit::new("fixture:compact-crash", json!({}), &s);
        let revision = s.revision.get() + 1;
        let record = Record {
            format: 1,
            baseline: None,
            baseline_revision: data["baseline_revision"].as_i64().unwrap(),
            previous_kernel_revision: data["previous_revision"].as_i64(),
            event_count: 1,
            steps: vec![CompactStep {
                preparation: Delta::between(&before, &before),
                event,
                after: Delta::between(&before, &after),
            }],
            settle: None,
            final_delta: Delta::between(&after, &after),
        };
        commit.result = json!({"kernel_delta":record,"kernel_event":event});
        commit.rows.push(RowChange::Put {
            table: "nodes".into(),
            id: "0".into(),
            value: json!(after.nodes[0]),
        });
        commit.rows.push(RowChange::Put {table:"kernel_replay_state".into(),id:"cursor".into(),value:json!({"index":2,"baseline_revision":data["baseline_revision"],"last_revision":revision})});
        let point = std::env::var("TL3_COMPACT_CRASH_POINT").unwrap();
        store.set_boundary_hook(move |at, _, _| {
            if format!("{at:?}") == point {
                println!("TL3_COMPACT_CRASH_READY");
                std::io::stdout().flush().unwrap();
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).unwrap();
            }
        });
        store.commit(&commit).unwrap();
        return;
    }
    let mut db = support::Database::start();
    let root = tempfile::tempdir().unwrap();
    for point in ["EventWritten", "RowsWritten", "BeforeCommit", "AfterCommit"] {
        let settings = db.create("compact_crash");
        let mut controller = Controller::open(
            Store::open(&settings).unwrap(),
            adapters::simulation(root.path()).unwrap(),
            Bootstrap {
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
            },
        )
        .unwrap();
        controller
            .intake(LabelledIssue {
                delivery: format!("fixture:{point}"),
                owner: "owner".into(),
                repo: "sandbox".into(),
                installation: 1,
                number: 42,
                title: "Crash fixture".into(),
                actor: 42,
                label: "tl3".into(),
                labels: vec![],
            })
            .unwrap();
        let before = controller.world().unwrap();
        let snapshot = controller.snapshot().unwrap();
        let mut reader = Reader::open(&settings).unwrap();
        let events = reader.events_after(tl3_store::Revision::ZERO, 128).unwrap();
        let last = events
            .iter()
            .rev()
            .find(|e| e.receipt.result.get("kernel_delta").is_some())
            .unwrap();
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
        let config = root.path().join(format!("{point}.toml"));
        std::fs::write(&config, source).unwrap();
        let data = root.path().join(format!("{point}.json"));
        std::fs::write(&data,serde_json::to_vec(&json!({"before":before,"previous_revision":last.receipt.revision.get(),"baseline_revision":last.receipt.result["kernel_delta"]["baseline_revision"]})).unwrap()).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "compact_commit_crash_safety", "--nocapture"])
            .env("TL3_COMPACT_CRASH_CONFIG", config)
            .env("TL3_COMPACT_CRASH_DATA", data)
            .env("TL3_COMPACT_CRASH_POINT", point)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let output = child.stdout.take().unwrap();
        let mut ready = false;
        for line in std::io::BufReader::new(output).lines() {
            if line.unwrap().contains("TL3_COMPACT_CRASH_READY") {
                ready = true;
                break;
            }
        }
        assert!(ready, "child never reached {point}");
        child.kill().unwrap();
        let status = child.wait().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(9));
        }
        let recovered = reader.load().unwrap();
        assert_eq!(
            recovered.revision.get(),
            snapshot.revision.get() + i64::from(point == "AfterCommit")
        );
        let report = replay::run(&mut reader, None).unwrap();
        assert_eq!(report.steps, if point == "AfterCommit" { 2 } else { 1 });
        println!("compact crash {point}: SIGKILL recovered a verified prefix");
    }
}

/// #783: malformed/truncated delta records fail at their revision, never pass.
#[test]
fn incomplete_delta_reports_unverifiable_revision() {
    let mut db = support::Database::start();
    let settings = db.create("compact_incomplete");
    let root = tempfile::tempdir().unwrap();
    let _controller = Controller::open(
        Store::open(&settings).unwrap(),
        adapters::simulation(root.path()).unwrap(),
        Bootstrap {
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
        },
    )
    .unwrap();
    let mut store = Store::open(&settings).unwrap();
    let snapshot = store.load().unwrap();
    let mut bad = Commit::new("fixture:truncated", json!({}), &snapshot);
    bad.result = json!({"kernel_delta":{"format":1}});
    let receipt = store.commit(&bad).unwrap();
    let error = replay::run(&mut Reader::open(&settings).unwrap(), None)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(&format!(
            "unverifiable from revision {}",
            receipt.revision.get()
        )),
        "{error}"
    );
}
