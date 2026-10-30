use super::*;
use tl3_store::Reader;

fn last_progress(settings: &tl3_config::Store) -> Option<String> {
    let mut reader = Reader::open(settings).unwrap();
    let mut tx = reader.transaction().unwrap();
    tl3_controller::status::load_request_details(&mut tx, &[NodeId(0)])
        .unwrap()
        .remove(&NodeId(0))
        .unwrap()
        .last_progress
}

fn seed(store: &mut Store, config: &tl3_config::Config, key: &str, state: EffectState) -> Delivery {
    let mut run = delivery();
    run.key = key.into();
    watch_run(store, &run);
    let mut transitions = vec![EffectTransition {
        key: key.into(),
        from: EffectState::Intended,
        to: EffectState::Sent,
    }];
    if state != EffectState::Sent {
        transitions.push(EffectTransition {
            key: key.into(),
            from: EffectState::Sent,
            to: state,
        });
    }
    save(store, vec![], transitions, vec![]);
    unwatch(store, &run);
    store.claim_worker(key, "0", 0, 0).unwrap();
    let attempt = root_for(config, key).join("attempt-00000000");
    fs::create_dir_all(&attempt).unwrap();
    for name in ["claim.json", "process.json"] {
        fs::write(attempt.join(name), r#"{"cancelled":true}"#).unwrap();
    }
    run
}

pub(super) fn summaries_and_states(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "normal");
    let mut store = Store::open(settings).unwrap();
    save(
        &mut store,
        vec![RowChange::Put {
            table: "nodes".into(),
            id: "0".into(),
            value: json!({"phase":"Making"}),
        }],
        vec![],
        vec![],
    );
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let uncertain = seed(&mut store, &config, "uncertain", EffectState::Uncertain);
    let sent = seed(&mut store, &config, "sent", EffectState::Sent);
    let cancelling = seed(&mut store, &config, "cancelling", EffectState::Cancelling);
    let cancelled = seed(&mut store, &config, "cancelled", EffectState::Cancelling);
    save(
        &mut store,
        vec![],
        vec![EffectTransition {
            key: cancelled.key.clone(),
            from: EffectState::Cancelling,
            to: EffectState::Cancelled,
        }],
        vec![],
    );
    let failed = seed(&mut store, &config, "failed", EffectState::Failed);
    let confirmed = seed(&mut store, &config, "confirmed", EffectState::Confirmed);
    // Retire in reverse progress order. Older evidence cannot replace the last progress.
    store
        .record_worker_progress(&failed.key, 0, 20, "model_turn")
        .unwrap();
    store
        .record_worker_progress(&confirmed.key, 0, 20, "tool_result")
        .unwrap();
    let mut connection = postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .dbname(&settings.database)
        .user(&settings.user)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap();
    connection.execute("UPDATE tl3_worker_progress SET last_progress_ms = CASE WHEN run_key = 'failed' THEN 1000 ELSE 2000 END WHERE run_key IN ('failed', 'confirmed')", &[]).unwrap();
    save(
        &mut store,
        vec![RowChange::Put {
            table: "watches".into(),
            id: failed.key.clone(),
            value: json!(failed),
        }],
        vec![],
        vec![],
    );
    let expected = store
        .worker_progress(None)
        .unwrap()
        .into_iter()
        .find(|p| p.key == confirmed.key)
        .unwrap()
        .last_progress_ms
        .unwrap();
    supervisor.retire(&mut store).unwrap();
    assert!(root_for(&config, &failed.key).exists());
    unwatch(&mut store, &failed);
    supervisor.retire(&mut store).unwrap();
    for run in [&cancelled, &failed, &confirmed] {
        assert!(!root_for(&config, &run.key).exists());
        assert!(!store.worker_reconcilable(&run.key).unwrap());
    }
    for run in [&uncertain, &sent, &cancelling] {
        assert!(root_for(&config, &run.key).exists());
        assert!(store.worker_reconcilable(&run.key).unwrap());
        store.forget_worker(&run.key).unwrap();
    }
    assert_eq!(store.worker_progress(None).unwrap().len(), 3);
    assert_eq!(
        last_progress(settings),
        None,
        "an active run with no progress takes precedence"
    );
    // Resolve the remaining deliveries without new progress.
    save(
        &mut store,
        vec![],
        vec![
            EffectTransition {
                key: uncertain.key.clone(),
                from: EffectState::Uncertain,
                to: EffectState::Confirmed,
            },
            EffectTransition {
                key: sent.key.clone(),
                from: EffectState::Sent,
                to: EffectState::Confirmed,
            },
            EffectTransition {
                key: cancelling.key.clone(),
                from: EffectState::Cancelling,
                to: EffectState::Cancelled,
            },
        ],
        vec![],
    );
    supervisor.retire(&mut store).unwrap();
    assert!(store.worker_progress(None).unwrap().is_empty());
    let time = chrono::DateTime::from_timestamp_millis(expected).unwrap();
    assert_eq!(
        last_progress(settings).unwrap(),
        format!(
            "Tool result received at {}",
            time.format("%Y-%m-%d %H:%M:%S UTC")
        )
    );
    let mut reader = Reader::open(settings).unwrap();
    let mut tx = reader.transaction().unwrap();
    let rows = tx
        .query("SELECT unit, last_progress_ms FROM tl3_worker_summary", &[])
        .unwrap();
    assert_eq!(rows.len(), 1, "one summary for many retired runs");
    assert_eq!(rows[0].get::<_, String>(0), "tool_result");
    assert_eq!(rows[0].get::<_, i64>(1), expected);
    tx.commit().unwrap();
    // Closing deletes the summary atomically and later cleanup cannot recreate it.
    save(
        &mut store,
        vec![RowChange::Put {
            table: "nodes".into(),
            id: "0".into(),
            value: json!({"phase":{"Closed":"Stopped"}}),
        }],
        vec![],
        vec![],
    );
    assert_eq!(last_progress(settings), None);
    let late = seed(&mut store, &config, "late", EffectState::Confirmed);
    store
        .record_worker_progress(&late.key, 0, 20, "model_turn")
        .unwrap();
    supervisor.retire(&mut store).unwrap();
    assert_eq!(last_progress(settings), None);
    assert_eq!(
        reader
            .transaction()
            .unwrap()
            .query_one("SELECT count(*) FROM tl3_worker_summary", &[])
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

pub(super) fn blocked_identities(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "normal");
    let mut store = Store::open(settings).unwrap();
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    for (key, record) in [
        ("corrupt", "{".into()),
        (
            "foreign",
            json!({"pid":1,"start":1,"boot":"old","machine":"another-machine"}).to_string(),
        ),
    ] {
        let run = seed(&mut store, &config, key, EffectState::Confirmed);
        let path = root_for(&config, &run.key).join("attempt-00000000/process.json");
        fs::write(&path, record).unwrap();
        supervisor.retire(&mut store).unwrap();
        assert!(path.exists(), "unknown identity cannot be proven dead");
        assert!(
            store.retired_workers().unwrap().is_empty(),
            "later drains skip a blocked identity"
        );
    }
    let mut restarted = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    restarted.retire(&mut store).unwrap();
    assert_eq!(store.worker_progress(None).unwrap().len(), 2);
}

pub(super) fn io_child(settings: &tl3_config::Store, root: &Path) {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    let config = config(settings, root, "normal");
    let supervisor = Supervisor::new(&config, &root.join("fakes")).unwrap();
    let mut source = supervisor.source();
    let mut store = Store::open(settings).unwrap();
    let (tx, _) = mpsc::channel();
    source.poll_once(&mut store, &tx).unwrap();
    assert_eq!(source.active_runs(), 1);
    let run = store_worker_run(settings);
    let attempt = root_for(&config, &run.key).join("attempt-00000000");
    // Force a live refresh after startup, then exhaust only this child's descriptors.
    let claim = fs::read(attempt.join("claim.json")).unwrap();
    fs::write(attempt.join("claim.json"), claim).unwrap();
    let original = getrlimit(Resource::Nofile);
    setrlimit(
        Resource::Nofile,
        Rlimit {
            current: Some(64),
            ..original
        },
    )
    .unwrap();
    let mut files = Vec::new();
    loop {
        match fs::File::open("/dev/null") {
            Ok(file) => files.push(file),
            Err(error) => {
                assert_eq!(error.raw_os_error(), Some(24));
                break;
            }
        }
    }
    let result = source.poll_once(&mut store, &tx);
    drop(files);
    setrlimit(Resource::Nofile, original).unwrap();
    assert!(
        matches!(result, Err(tl3_controller::Error::Io(error)) if error.raw_os_error() == Some(24)),
        "EMFILE must fail the source loudly"
    );
    let mut restarted = supervisor.source();
    restarted.poll_once(&mut store, &tx).unwrap();
    assert_eq!(
        restarted.active_runs(),
        1,
        "restart reattaches the live run"
    );
}

pub(super) fn transient_io(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "blocked");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let run = delivery();
    watch_run(&mut store, &run);
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    supervisor.execute(&run).unwrap();
    wait(|| root.path().join("entered").exists());
    let path = root.path().join("settings.json");
    fs::write(&path, serde_json::to_vec(&json!({"host":settings.host,"port":settings.port,"database":settings.database,"user":settings.user,"password_file":settings.password_file,"connect_timeout_secs":settings.connect_timeout_secs})).unwrap()).unwrap();
    let result = Command::new(env::current_exe().unwrap())
        .arg("--source-io-child")
        .arg(path)
        .arg(root.path())
        .status()
        .unwrap();
    supervisor.stop_run(&run.key, 0).unwrap();
    assert!(result.success());
}

pub(super) fn session_guard(settings: &tl3_config::Store, guard: &str) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "normal");
    let mut store = Store::open(settings).unwrap();
    save(
        &mut store,
        vec![RowChange::Put {
            table: "nodes".into(),
            id: "0".into(),
            value: json!({"phase":{"Closed":"Stopped"}}),
        }],
        vec![],
        vec![],
    );
    let session = config.workers.runs.join("sessions/0/plan-0/attempt-1.json");
    fs::create_dir_all(session.parent().unwrap()).unwrap();
    fs::write(&session, "saved conversation").unwrap();
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let run = delivery();
    match guard {
        "inbox" => save(
            &mut store,
            vec![RowChange::Put {
                table: "inbox".into(),
                id: "pending".into(),
                value: json!({"pending":true}),
            }],
            vec![],
            vec![],
        ),
        "effect" => save(
            &mut store,
            vec![],
            vec![],
            vec![NewEffect {
                key: run.key.clone(),
                kind: "Workers".into(),
                payload: json!(run),
            }],
        ),
        "progress" => {
            seed(&mut store, &config, &run.key, EffectState::Confirmed);
            store.block_worker_retirement(&run.key).unwrap();
        }
        _ => panic!("unknown session guard"),
    }
    assert!(!store.can_retire_worker_session("0").unwrap());
    supervisor.retire(&mut store).unwrap();
    assert!(
        session.exists(),
        "{guard} alone must retain a closed node's conversation"
    );
    match guard {
        "inbox" => save(
            &mut store,
            vec![RowChange::Delete {
                table: "inbox".into(),
                id: "pending".into(),
            }],
            vec![],
            vec![],
        ),
        "effect" => save(
            &mut store,
            vec![],
            vec![
                EffectTransition {
                    key: run.key.clone(),
                    from: EffectState::Intended,
                    to: EffectState::Sent,
                },
                EffectTransition {
                    key: run.key.clone(),
                    from: EffectState::Sent,
                    to: EffectState::Confirmed,
                },
            ],
            vec![],
        ),
        "progress" => {
            fs::remove_dir_all(root_for(&config, &run.key)).unwrap();
            store.forget_worker(&run.key).unwrap();
        }
        _ => unreachable!(),
    }
    assert!(store.can_retire_worker_session("0").unwrap());
    supervisor.retire(&mut store).unwrap();
    assert!(!session.exists());
}

pub(super) fn session_unlink_error(settings: &tl3_config::Store) {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "normal");
    let mut store = Store::open(settings).unwrap();
    for node in ["0", "1"] {
        save(
            &mut store,
            vec![RowChange::Put {
                table: "nodes".into(),
                id: node.into(),
                value: json!({"phase":{"Closed":"Stopped"}}),
            }],
            vec![],
            vec![],
        );
        let session = config.workers.runs.join("sessions").join(node);
        fs::create_dir_all(&session).unwrap();
        fs::write(session.join("attempt.json"), "saved conversation").unwrap();
    }
    let retained = config.workers.runs.join("sessions/0");
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    fs::set_permissions(&retained, fs::Permissions::from_mode(0o500)).unwrap();
    let result = supervisor.retire(&mut store);
    fs::set_permissions(&retained, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        result.is_ok(),
        "an undeletable session must not fail idle drive_once"
    );
    assert!(retained.join("attempt.json").exists());
    assert!(
        !config.workers.runs.join("sessions/1").exists(),
        "other nodes still retire"
    );
    supervisor.retire(&mut store).unwrap();
    assert!(
        !retained.exists(),
        "a later idle pass retries retained sessions"
    );
}
