mod support;
use postgres::{Client, NoTls};
use serde_json::json;
use std::{
    env, fs,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use support::Database;
use tl3_store::*;

fn main() {
    let args: Vec<_> = env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--crash-writer") {
        crash_writer(&args);
        return;
    }
    let mut database = Database::start();
    for (name, test) in [
        (
            "atomic_commit_and_idempotency",
            atomic_commit_and_idempotency as fn(&tl3_config::Store),
        ),
        ("all_effect_transitions", all_effect_transitions),
        ("credit_and_interning", credit_and_interning),
        ("concurrent_writers", concurrent_writers),
        ("epochs_and_upgrades", epochs_and_upgrades),
        ("migrations_and_append_only", migrations_and_append_only),
        ("killed_writer_recovers", killed_writer_recovers),
        ("durability_settings", durability_settings),
        ("arrival_within_transaction", arrival_within_transaction),
    ] {
        let settings = database.create(name);
        test(&settings);
        println!("test {name} ... ok");
    }
    database.stop();
    println!("9 Postgres integration scenarios passed; no skips");
}

fn raw(settings: &tl3_config::Store) -> Client {
    postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect(NoTls)
        .unwrap()
}

fn put(id: &str, value: serde_json::Value) -> RowChange {
    RowChange::Put {
        table: "nodes".into(),
        id: id.into(),
        value,
    }
}

fn arrival_within_transaction(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let snapshot = store.load().unwrap();
    let mut change = Commit::new("arrivals", json!(null), &snapshot);
    for key in ["effect:2", "effect:12", "effect:1"] {
        change.rows.push(RowChange::Put {
            table: "inbox".into(),
            id: key.into(),
            value: json!(key),
        });
    }
    store.commit(&change).unwrap();
    let before = store.load().unwrap();
    assert_eq!(
        before
            .rows
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        ["effect:2", "effect:12", "effect:1"]
    );
    assert!(
        before
            .rows
            .windows(2)
            .all(|pair| pair[0].arrival < pair[1].arrival)
    );
    let mut update = Commit::new("update-arrival", json!(null), &before);
    update.rows.push(RowChange::Put {
        table: "inbox".into(),
        id: "effect:2".into(),
        value: json!("updated"),
    });
    store.commit(&update).unwrap();
    let after = store.load().unwrap();
    assert_eq!(
        after.rows.iter().map(|r| r.arrival).collect::<Vec<_>>(),
        before.rows.iter().map(|r| r.arrival).collect::<Vec<_>>()
    );
    assert_eq!(
        Store::open_read_only(settings).unwrap().load().unwrap(),
        after
    );
}

fn effect(key: &str) -> NewEffect {
    NewEffect {
        key: key.into(),
        kind: "start-agent".into(),
        payload: json!({"operation": key}),
    }
}

fn seed(store: &mut Store) -> Commit {
    let snapshot = store.load().unwrap();
    let mut change = Commit::new(
        "event-1",
        json!({"request": 1, "large_number": 1.0e18}),
        &snapshot,
    );
    change.rows = vec![
        put("1", json!({"phase": "Making"})),
        put("2", json!([1, 2])),
    ];
    change.effects = vec![effect("effect-1")];
    change.credit = vec![CreditEntry {
        root: "1".into(),
        node: "1".into(),
        reason: "initial".into(),
        amount: 4,
    }];
    change.result = json!({"accepted": 1, "large_number": 1.0e18});
    change
}

fn atomic_commit_and_idempotency(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let initial = store.load().unwrap();
    let mut change = seed(&mut store);
    // Fail after the event, rows and first effect have been inserted.
    change.effects.push(effect("effect-1"));
    assert!(matches!(store.commit(&change), Err(Error::Database(_))));
    assert_eq!(store.load().unwrap(), initial);
    assert!(store.events_after(Revision::ZERO, 100).unwrap().is_empty());
    change.effects.pop();
    let receipt = store.commit(&change).unwrap();
    let committed = store.load().unwrap();
    assert_eq!(committed.rows.len(), 2);
    assert_eq!(committed.pending_effects.len(), 1);
    assert_eq!(committed.credit.len(), 1);
    assert!(
        committed
            .rows
            .iter()
            .all(|row| row.version == receipt.revision)
    );
    change.rows = vec![put("1", json!("must never land"))];
    change.result = json!("different redelivery result");
    assert_eq!(store.commit(&change).unwrap(), receipt);
    assert_eq!(store.load().unwrap(), committed);
    change.event = json!({"request": 2});
    assert!(matches!(store.commit(&change), Err(Error::EventKeyReused)));
    assert_eq!(store.load().unwrap(), committed);
    let events = store.events_after(Revision::ZERO, 100).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].receipt, receipt);
    assert_eq!(events[0].payload["request"], json!(1));
    assert!(
        store
            .events_after(receipt.revision, 100)
            .unwrap()
            .is_empty()
    );
    let mut delete = Commit::new("delete", json!(null), &committed);
    delete.rows = vec![RowChange::Delete {
        table: "nodes".into(),
        id: "2".into(),
    }];
    store.commit(&delete).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.rows.len(), 1);
    assert_eq!(loaded.rows[0], committed.rows[0]);
    drop(store);
    assert_eq!(Store::open(settings).unwrap().load().unwrap(), loaded);
}

fn transition(
    store: &mut Store,
    key: &str,
    from: EffectState,
    to: EffectState,
    event: &str,
) -> Result<CommitReceipt> {
    let snapshot = store.load()?;
    let mut change = Commit::new(event, json!(null), &snapshot);
    change.transitions.push(EffectTransition {
        key: key.into(),
        from,
        to,
    });
    store.commit(&change)
}

fn all_effect_transitions(settings: &tl3_config::Store) {
    use EffectState::*;
    let states = [
        Intended, Sent, Uncertain, Confirmed, Failed, Cancelling, Cancelled,
    ];
    let legal = [
        (Intended, Sent),
        (Intended, Cancelled),
        (Sent, Confirmed),
        (Sent, Intended),
        (Sent, Failed),
        (Sent, Uncertain),
        (Sent, Cancelling),
        (Uncertain, Confirmed),
        (Uncertain, Intended),
        (Uncertain, Cancelling),
        (Cancelling, Confirmed),
        (Cancelling, Cancelled),
    ];
    let mut store = Store::open(settings).unwrap();
    for from in states {
        for to in states {
            let key = format!("{from:?}-{to:?}");
            let snapshot = store.load().unwrap();
            let mut create = Commit::new(format!("create-{key}"), json!(null), &snapshot);
            create.effects.push(effect(&key));
            store.commit(&create).unwrap();
            let path: &[EffectState] = match from {
                Intended => &[],
                Sent => &[Sent],
                Uncertain => &[Sent, Uncertain],
                Confirmed => &[Sent, Confirmed],
                Failed => &[Sent, Failed],
                Cancelling => &[Sent, Cancelling],
                Cancelled => &[Cancelled],
            };
            let mut previous = Intended;
            for (i, &next) in path.iter().enumerate() {
                transition(
                    &mut store,
                    &key,
                    previous,
                    next,
                    &format!("setup-{key}-{i}"),
                )
                .unwrap();
                previous = next;
            }
            let before = store.load().unwrap();
            let result = transition(&mut store, &key, from, to, &format!("test-{key}"));
            if legal.contains(&(from, to)) {
                result.unwrap();
                assert_eq!(store.effect(&key).unwrap().unwrap().state, to);
            } else {
                assert!(
                    matches!(result, Err(Error::IllegalTransition { .. })),
                    "{from:?} -> {to:?}"
                );
                assert_eq!(store.load().unwrap(), before);
                // The migration rejects the same illegal changes independently of Rust.
                if from != to {
                    assert!(raw(settings).execute("UPDATE tl3_effects SET state = $1::text::tl3_effect_state WHERE effect_key = $2", &[&format!("{to:?}"), &key]).is_err());
                }
            }
            let persisted = store.effect(&key).unwrap().unwrap();
            assert_eq!(
                store
                    .load()
                    .unwrap()
                    .pending_effects
                    .iter()
                    .any(|effect| effect.key == key),
                persisted.state.is_pending()
            );
        }
    }
    assert!(matches!(
        transition(&mut store, "missing", Intended, Sent, "missing"),
        Err(Error::EffectConflict)
    ));
    let snapshot = store.load().unwrap();
    let mut create = Commit::new("cas-create", json!(null), &snapshot);
    create.effects.push(effect("cas"));
    store.commit(&create).unwrap();
    assert!(matches!(
        transition(&mut store, "cas", Sent, Confirmed, "cas-mismatch"),
        Err(Error::EffectConflict)
    ));
}

fn credit_and_interning(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let seed = seed(&mut store);
    store.commit(&seed).unwrap();
    let snapshot = store.load().unwrap();
    let mut duplicate = Commit::new("credit-duplicate", json!(null), &snapshot);
    duplicate.credit = seed.credit.clone();
    store.commit(&duplicate).unwrap();
    assert_eq!(store.load().unwrap().credit, snapshot.credit);
    let before = store.load().unwrap();
    let mut conflict = Commit::new("credit-conflict", json!(null), &before);
    conflict.credit = seed.credit;
    conflict.credit[0].amount += 1;
    conflict.rows.push(put("wrong", json!(true)));
    assert!(matches!(
        store.commit(&conflict),
        Err(Error::CreditConflict)
    ));
    assert_eq!(store.load().unwrap(), before);
    let repo = store.intern_repo("RobTand", "TL3").unwrap();
    assert_eq!(repo, store.intern_repo("robtand", "tl3").unwrap());
    let other = store.intern_repo("RobTand", "other").unwrap();
    let sha = "a".repeat(40);
    let commit = store.intern_commit("node-1", &sha).unwrap();
    assert_eq!(
        commit,
        store.intern_commit("node-1", &sha.to_uppercase()).unwrap()
    );
    assert_ne!(commit, store.intern_commit("node-2", &sha).unwrap());
    let path = store.intern_path(repo, "src/lib.rs").unwrap();
    assert_eq!(path, store.intern_path(repo, "src/lib.rs").unwrap());
    assert_ne!(path, store.intern_path(other, "src/lib.rs").unwrap());
    assert_ne!(path, store.intern_path(repo, "Src/lib.rs").unwrap());
    let issue = store.intern_issue(repo, 55).unwrap();
    assert_eq!(issue, store.intern_issue(repo, 55).unwrap());
    assert_ne!(issue, store.intern_issue(other, 55).unwrap());
    let pr = store.intern_pull_request(repo, 55).unwrap();
    assert_eq!(pr, store.intern_pull_request(repo, 55).unwrap());
    assert_ne!(pr.get(), issue.get());
    assert!(store.intern_issue(repo, 0).is_err());
    assert!(store.intern_commit("node-1", "bad").is_err());
    let mut client = raw(settings);
    client
        .query_one("SELECT setval('tl3_intern_id_seq', 2147483648)", &[])
        .unwrap();
    let large = store.intern_repo("RobTand", "large-id").unwrap();
    assert!(large.get() > i64::from(i32::MAX));
    let sequence = || {
        let row = raw(settings)
            .query_one("SELECT last_value, is_called FROM tl3_intern_id_seq", &[])
            .unwrap();
        (row.get::<_, i64>(0), row.get::<_, bool>(1))
    };
    let before_lookups = sequence();
    client.batch_execute("CREATE FUNCTION test_no_intern_update() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'intern lookups must not update rows'; END $$; CREATE TRIGGER test_no_intern_update BEFORE UPDATE ON tl3_intern FOR EACH ROW EXECUTE FUNCTION test_no_intern_update()").unwrap();
    for _ in 0..1_000 {
        assert_eq!(store.intern_repo("RobTand", "TL3").unwrap(), repo);
        assert_eq!(store.intern_commit("node-1", &sha).unwrap(), commit);
        assert_eq!(store.intern_path(repo, "src/lib.rs").unwrap(), path);
        assert_eq!(store.intern_issue(repo, 55).unwrap(), issue);
        assert_eq!(store.intern_pull_request(repo, 55).unwrap(), pr);
    }
    assert_eq!(sequence(), before_lookups);
    let before = store.load().unwrap();
    assert!(
        before
            .interned
            .iter()
            .any(|value| value.repo_id() == Some(repo))
    );
    drop(store);
    let mut reopened = Store::open(settings).unwrap();
    assert_eq!(reopened.load().unwrap(), before);
    assert_eq!(reopened.intern_repo("RobTand", "large-id").unwrap(), large);
    assert_eq!(reopened.intern_commit("node-1", &sha).unwrap(), commit);
}

fn concurrent_writers(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let initial = store.load().unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|i| {
            let settings = settings.clone();
            let snapshot = initial.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let mut store = Store::open(&settings).unwrap();
                let mut change = Commit::new(format!("concurrent-{i}"), json!(i), &snapshot);
                change.rows.push(put("1", json!(i)));
                barrier.wait();
                store.commit(&change)
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(Error::Stale { .. })))
            .count(),
        1
    );
    assert_eq!(store.events_after(Revision::ZERO, 10).unwrap().len(), 1);
    let snapshot = store.load().unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let settings = settings.clone();
            let snapshot = snapshot.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let mut store = Store::open(&settings).unwrap();
                let change = Commit::new("concurrent-duplicate", json!("same event"), &snapshot);
                barrier.wait();
                store.commit(&change).unwrap()
            })
        })
        .collect();
    let receipts: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(receipts[0], receipts[1]);
    assert_eq!(store.events_after(Revision::ZERO, 10).unwrap().len(), 2);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let settings = settings.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let mut store = Store::open(&settings).unwrap();
                barrier.wait();
                store.intern_repo("RobTand", "concurrent").unwrap()
            })
        })
        .collect();
    let ids: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert!(ids.iter().all(|id| *id == ids[0]));
    assert_eq!(
        raw(settings)
            .query_one("SELECT count(*) FROM tl3_intern WHERE kind = 'Repo' AND scope = 'robtand' AND value = 'concurrent'", &[])
            .unwrap()
            .get::<_, i64>(0),
        1
    );
}

fn epochs_and_upgrades(settings: &tl3_config::Store) {
    use EffectState::*;
    let mut store = Store::open(settings).unwrap();
    let seed = seed(&mut store);
    let original = store.commit(&seed).unwrap();
    transition(&mut store, "effect-1", Intended, Sent, "picked-up").unwrap();
    let active = store.load().unwrap();
    let mut upgrade = Commit::new("upgrade", json!(null), &active);
    upgrade.next_epoch = Some(Epoch::try_from(1).unwrap());
    assert!(matches!(
        store.commit(&upgrade),
        Err(Error::ActiveDeliveries)
    ));
    assert_eq!(store.load().unwrap(), active);
    transition(&mut store, "effect-1", Sent, Uncertain, "crashed").unwrap();
    transition(&mut store, "effect-1", Uncertain, Intended, "absent").unwrap();
    let before = store.load().unwrap();
    upgrade.expected_revision = before.revision;
    upgrade.rows.push(put("1", json!({"schema": 2})));
    store.commit(&upgrade).unwrap();
    let after = store.load().unwrap();
    assert_eq!(after.epoch.get(), 1);
    assert_eq!(after.pending_effects[0].key, "effect-1");
    assert_eq!(after.pending_effects[0].epoch, after.epoch);
    let mut stale = Commit::new("old-controller", json!(null), &after);
    stale.epoch = Epoch::ZERO;
    assert!(matches!(store.commit(&stale), Err(Error::Stale { .. })));
    assert_eq!(store.commit(&seed).unwrap(), original);
    assert_eq!(store.load().unwrap(), after);
    let mut skip = Commit::new("skip-epoch", json!(null), &after);
    skip.next_epoch = Some(Epoch::try_from(3).unwrap());
    assert!(matches!(store.commit(&skip), Err(Error::Invalid(_))));
    assert_eq!(Store::open(settings).unwrap().load().unwrap(), after);
}

fn migrations_and_append_only(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let seed = seed(&mut store);
    store.commit(&seed).unwrap();
    drop(store);
    let _store = Store::open(settings).unwrap();
    let mut client = raw(settings);
    assert_eq!(
        client
            .query_one("SELECT count(*) FROM tl3_schema_migrations", &[])
            .unwrap()
            .get::<_, i64>(0),
        2
    );
    for table in ["tl3_events", "tl3_credit"] {
        for sql in [
            format!("DELETE FROM {table}"),
            format!("TRUNCATE {table} CASCADE"),
        ] {
            assert!(client.batch_execute(&sql).is_err());
        }
    }
    assert!(
        client
            .batch_execute("UPDATE tl3_events SET result = 'null'::jsonb")
            .is_err()
    );
    assert!(
        client
            .batch_execute("UPDATE tl3_credit SET amount = 10")
            .is_err()
    );
    client
        .execute(
            "INSERT INTO tl3_schema_migrations VALUES (999, 'future')",
            &[],
        )
        .unwrap();
    assert!(matches!(
        Store::open(settings),
        Err(Error::MigrationMismatch)
    ));
    client
        .execute("DELETE FROM tl3_schema_migrations WHERE version = 999", &[])
        .unwrap();
    client
        .execute("UPDATE tl3_schema_migrations SET sql = 'changed'", &[])
        .unwrap();
    assert!(matches!(
        Store::open(settings),
        Err(Error::MigrationMismatch)
    ));
}

fn durability_settings(settings: &tl3_config::Store) {
    let _store = Store::open(settings).unwrap();
    let mut client = raw(settings);
    client
        .batch_execute("ALTER DATABASE durability_settings SET synchronous_commit = 'off'")
        .unwrap();
    assert_eq!(
        raw(settings)
            .query_one("SHOW synchronous_commit", &[])
            .unwrap()
            .get::<_, &str>(0),
        "off"
    );
    client.batch_execute("CREATE FUNCTION test_check_durable() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF current_setting('synchronous_commit') <> 'on' THEN RAISE EXCEPTION 'commit must flush WAL'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_check_durable BEFORE INSERT ON tl3_events FOR EACH ROW EXECUTE FUNCTION test_check_durable()").unwrap();
    let mut durable = Store::open(settings).unwrap();
    let snapshot = durable.load().unwrap();
    durable
        .commit(&Commit::new("durable", json!(null), &snapshot))
        .unwrap();
    drop(durable);
    let mut unsafe_database = Database::start_with_fsync(false);
    let unsafe_settings = unsafe_database.create("unsafe_durability");
    assert!(matches!(
        Store::open(&unsafe_settings),
        Err(Error::Invalid("store requires Postgres fsync = on"))
    ));
    unsafe_database.stop();
}

struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for crash injection point"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn killed_writer_recovers(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let mut client = raw(settings);
    client.batch_execute("CREATE FUNCTION test_block_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.node_id = 'block-crash' THEN PERFORM pg_advisory_xact_lock(550055); END IF; RETURN NEW; END $$; CREATE TRIGGER test_block_commit BEFORE INSERT ON tl3_credit FOR EACH ROW EXECUTE FUNCTION test_block_commit()").unwrap();
    client
        .query_one("SELECT pg_advisory_lock(550055)", &[])
        .unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let ready = scratch.path().join("first-commit-ready");
    let mut child = KillOnDrop(
        Command::new(env::current_exe().unwrap())
            .arg("--crash-writer")
            .arg(settings.port.to_string())
            .arg(&settings.database)
            .arg(settings.password_file.as_ref().unwrap())
            .arg(&ready)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    until(|| ready.exists());
    let committed = store.load().unwrap();
    assert_eq!(committed.revision.get(), 1);
    assert_eq!(committed.rows.len(), 2);
    until(|| {
        client.query_one("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = $1 AND wait_event_type = 'Lock' AND wait_event = 'advisory' AND query LIKE 'INSERT INTO tl3_credit%')", &[&settings.database]).unwrap().get(0)
    });
    // The second transaction has written its event, rows and outbox, and is blocked in credit.
    assert_eq!(store.load().unwrap(), committed);
    child.0.kill().unwrap();
    let status = child.0.wait().unwrap();
    assert!(!status.success());
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
    }
    client
        .query_one("SELECT pg_advisory_unlock(550055)", &[])
        .unwrap();
    drop(store);
    let mut recovered = Store::open(settings).unwrap();
    assert_eq!(recovered.load().unwrap(), committed);
    assert!(recovered.effect("uncommitted-effect").unwrap().is_none());
    assert_eq!(
        recovered.events_after(Revision::ZERO, 100).unwrap().len(),
        1
    );
    // The killed event key is usable: its uncommitted insertion was rolled back too.
    let retry = second_commit(&committed);
    recovered.commit(&retry).unwrap();
    assert_eq!(recovered.load().unwrap().revision.get(), 2);
    assert_eq!(
        recovered.events_after(Revision::ZERO, 100).unwrap().len(),
        2
    );
}

fn second_commit(snapshot: &Snapshot) -> Commit {
    let mut change = Commit::new("event-killed", json!({"second": true}), snapshot);
    change.rows = vec![put("1", json!("uncommitted")), put("3", json!(true))];
    change.effects = vec![effect("uncommitted-effect")];
    change.credit = vec![CreditEntry {
        node: "block-crash".into(),
        root: "1".into(),
        reason: "attempt:2".into(),
        amount: -1,
    }];
    change
}

fn crash_writer(args: &[String]) {
    let settings = tl3_config::Store {
        port: args[2].parse().unwrap(),
        database: args[3].clone(),
        user: "postgres".into(),
        password_file: Some(args[4].clone().into()),
        ..tl3_config::Store::default()
    };
    let mut store = Store::open(&settings).unwrap();
    let first = seed(&mut store);
    store.commit(&first).unwrap();
    let snapshot = store.load().unwrap();
    fs::write(Path::new(&args[5]), b"committed").unwrap();
    store.commit(&second_commit(&snapshot)).unwrap();
    panic!("parent should kill the writer before the second commit returns");
}
