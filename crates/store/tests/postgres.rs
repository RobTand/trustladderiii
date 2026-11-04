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
    let cases = [
        (
            "ownership_release",
            ownership_release as fn(&tl3_config::Store),
        ),
        ("ownership_drop", ownership_drop),
        ("ownership_release_after_loss", ownership_release_after_loss),
        (
            "mvp_schema_6_upgrades_to_hindsight",
            mvp_schema_6_upgrades_to_hindsight as fn(&tl3_config::Store),
        ),
        ("worker_completion_facts", worker_completion_facts),
        ("review_fixes_worker_document", review_fixes_worker_document),
        (
            "atomic_commit_and_idempotency",
            atomic_commit_and_idempotency as fn(&tl3_config::Store),
        ),
        ("wait_clears_on_progress", wait_clears_on_progress),
        ("wait_resets_on_attempt", wait_resets_on_attempt),
        ("wait_resets_on_generation", wait_resets_on_generation),
        ("all_effect_transitions", all_effect_transitions),
        ("credit_and_interning", credit_and_interning),
        ("concurrent_writers", concurrent_writers),
        ("epochs_and_upgrades", epochs_and_upgrades),
        ("migrations_and_append_only", migrations_and_append_only),
        ("killed_writer_recovers", killed_writer_recovers),
        ("durability_settings", durability_settings),
        ("read_only_reader", read_only_reader),
        ("reader_schema_versions", reader_schema_versions),
        ("arrival_within_transaction", arrival_within_transaction),
        ("ingress_retirement", ingress_retirement),
        ("change_feed", change_feed),
        ("phase_entry_times", phase_entry_times),
        ("phase_backfill_from_schema_9", phase_backfill_from_schema_9),
        ("reader_role", reader_role),
        ("postmortem_records", postmortem_records),
        ("postmortem_results", postmortem_results),
        ("postmortem_retries_and_pace", postmortem_retries_and_pace),
        (
            "postmortem_migration_on_schema_12",
            postmortem_migration_on_schema_12,
        ),
        ("settings_editor", settings_editor),
        ("console_commands", console_commands),
        ("prompt_seeding", prompt_seeding),
        ("prompt_builtin_updates", prompt_builtin_updates),
        ("prompt_edits_and_revert", prompt_edits_and_revert),
        (
            "prompt_activation_needs_the_active_version",
            prompt_activation_needs_the_active_version,
        ),
        ("prompt_history_and_limits", prompt_history_and_limits),
        ("prompt_session_liveness", prompt_session_liveness),
        (
            "prompts_open_on_a_store_ahead",
            prompts_open_on_a_store_ahead,
        ),
        (
            "prompt_version_recorded_for_a_run",
            prompt_version_recorded_for_a_run,
        ),
        (
            "console_login_writes_only_prompts",
            console_login_writes_only_prompts,
        ),
        (
            "console_login_granted_before_migration",
            console_login_granted_before_migration,
        ),
    ];
    let selected: Vec<_> = cases
        .iter()
        .filter(|(name, _)| !args.get(1).is_some_and(|s| s == "--r3") || name.starts_with("wait_"))
        .collect();
    for (name, test) in &selected {
        let settings = if matches!(
            *name,
            "ownership_release"
                | "mvp_schema_6_upgrades_to_hindsight"
                | "phase_backfill_from_schema_9"
                | "postmortem_migration_on_schema_12"
                | "read_only_reader"
                | "reader_schema_versions"
                | "console_login_granted_before_migration"
                | "prompts_open_on_a_store_ahead"
        ) {
            database.create_uninitialized(name)
        } else {
            database.create(name)
        };
        test(&settings);
        println!("test {name} ... ok");
    }
    let settings = database.create("reader_restart");
    let before = Store::open(&settings).unwrap().load().unwrap();
    let mut reader = Reader::open(&settings).unwrap();
    database.restart();
    assert!(reader.load().is_err());
    assert_eq!(Reader::open(&settings).unwrap().load().unwrap(), before);
    println!("test reader_restart ... ok");
    database.stop();
    // reader_restart runs outside the list because it restarts Postgres.
    println!(
        "{} Postgres integration scenarios passed; no skips",
        selected.len() + 1
    );
}

fn migrate(settings: &tl3_config::Store) -> tl3_store::Result<Store> {
    let mut ownership = Ownership::claim(Store::connect(settings)?)?;
    ownership.migrate()?;
    ownership.release()?;
    Store::open(settings)
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

/// #785 / batch 10: a successor must not wait for the old backend to exit.
fn ownership_release(settings: &tl3_config::Store) {
    let mut owner = Ownership::claim(Store::connect(settings).unwrap()).unwrap();
    owner.migrate().unwrap();
    let mut observer = raw(settings);
    let pid = owner_pid(&mut observer);
    let owner = Ownership::claim(owner.into_store()).unwrap();
    assert!(
        Ownership::claim(Store::open(settings).unwrap()).is_err(),
        "live owners still refuse contenders"
    );
    // Keep the exact old session alive after release. This makes a missing
    // unlock deterministic, independently of when Postgres processes Terminate.
    let mut released = owner.release().unwrap();
    released.load().unwrap();
    assert_eq!(
        observer
            .query_one(
                "SELECT count(*) FROM pg_stat_activity WHERE pid = $1",
                &[&pid]
            )
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    assert_eq!(observer.query_one("SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND pid = $1 AND classid::bigint = 1414280003 AND objid::bigint = 1330533458 AND objsubid = 1 AND granted", &[&pid]).unwrap().get::<_, i64>(0), 0, "release must acknowledge the unlock while the old backend is still alive");
    let mut successor = Ownership::claim(Store::open(settings).unwrap()).unwrap();
    successor.ensure_held().unwrap();
    released.load().unwrap();
    successor.release().unwrap();
}

fn owner_pid(observer: &mut Client) -> i32 {
    observer.query_one("SELECT pid FROM pg_locks WHERE locktype = 'advisory' AND classid::bigint = 1414280003 AND objid::bigint = 1330533458 AND objsubid = 1 AND database = (SELECT oid FROM pg_database WHERE datname = current_database()) AND granted", &[]).unwrap().get(0)
}

/// #785: a failed release reports session loss rather than claiming success.
fn ownership_release_after_loss(settings: &tl3_config::Store) {
    let owner = Ownership::claim(Store::open(settings).unwrap()).unwrap();
    let mut observer = raw(settings);
    let pid = owner_pid(&mut observer);
    assert!(
        observer
            .query_one("SELECT pg_terminate_backend($1)", &[&pid])
            .unwrap()
            .get::<_, bool>(0)
    );
    assert!(matches!(owner.release(), Err(Error::OwnershipLost)));
}

/// #785: transfer keeps ownership; normal drop and unwinding release it.
fn ownership_drop(settings: &tl3_config::Store) {
    for panic in [false, true] {
        let owner = Ownership::claim(Store::open(settings).unwrap()).unwrap();
        let mut observer = raw(settings);
        let pid = owner_pid(&mut observer);
        let transferred = owner.into_store();
        assert!(
            Ownership::claim(Store::open(settings).unwrap()).is_err(),
            "transferring the session must not unlock it"
        );
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = transferred;
            assert_eq!(owner_pid(&mut observer), pid);
            assert!(!panic, "exercise ownership cleanup during unwinding");
        }));
        assert_eq!(result.is_err(), panic);
        let successor = Ownership::claim(Store::open(settings).unwrap()).unwrap();
        successor.release().unwrap();
    }
}

fn review_fixes_worker_document(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    assert_eq!(store.worker_document("0", "review_fixes").unwrap(), None);
    let evidence = json!({
        "reviewed_sha": "a".repeat(40),
        "head_sha": "b".repeat(40),
        "findings": [{"id": "review-7-0", "report": "Clarify the wording", "reason": "Documentation only"}]
    });
    let mut change = Commit::new("fix-evidence", json!(null), &store.load().unwrap());
    change.rows.push(RowChange::Put {
        table: "review_fixes".into(),
        id: "0".into(),
        value: evidence.clone(),
    });
    store.commit(&change).unwrap();
    let mut recovered = Store::open(settings).unwrap();
    assert_eq!(
        recovered.worker_document("0", "review_fixes").unwrap(),
        Some(evidence)
    );
    assert!(recovered.worker_document("0", "not_a_document").is_err());
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
    assert_eq!(Reader::open(settings).unwrap().load().unwrap(), after);
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
    // #371: a newer release may add members to the same fact. The replay keeps
    // the stored receipt and changes nothing.
    let original = change.event.clone();
    change.event =
        json!({"request": 1, "large_number": 1.0e18, "model": "newer-field", "extra": null});
    assert_eq!(store.commit(&change).unwrap(), receipt);
    assert_eq!(store.load().unwrap(), committed);
    // A different fact under the key still stops the commit, and names the key.
    change.event = json!({"request": 2});
    let error = store.commit(&change).unwrap_err();
    assert!(
        matches!(&error, Error::EventKeyReused { key, stored, new }
            if key == "event-1" && stored.len() == 32 && new.len() == 32 && stored != new),
        "{error}"
    );
    assert!(error.to_string().contains("event-1"));
    // Dropping a member the stored fact had is a different fact too.
    change.event = json!({"request": 1});
    assert!(matches!(
        store.commit(&change),
        Err(Error::EventKeyReused { .. })
    ));
    change.event = original;
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
    for length in [40, 64] {
        let canonical = "a".repeat(length);
        assert!(store.intern_commit("node-1", &canonical).is_ok());
        assert!(
            store
                .intern_commit("node-1", &canonical.to_uppercase())
                .is_err(),
            "uppercase SHAs must be rejected"
        );
    }
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

fn mvp_schema_6_upgrades_to_hindsight(settings: &tl3_config::Store) {
    let mut client = raw(settings);
    // Freeze #115's deployed migration bytes independently of the current list.
    let deployed = [
        include_str!("../migrations/0001_store.sql"),
        include_str!("../migrations/0002_world_arrival.sql"),
        include_str!("../migrations/0003_workers.sql"),
        include_str!("../migrations/0004_ingress.sql"),
        include_str!("../migrations/0005_worker_retirement.sql"),
        include_str!("fixtures/mvp_schema_6.sql"),
    ];
    client
        .batch_execute(
            "CREATE TABLE tl3_schema_migrations (version integer PRIMARY KEY, sql text NOT NULL)",
        )
        .unwrap();
    for (index, sql) in deployed.iter().enumerate() {
        client.batch_execute(sql).unwrap();
        client
            .execute(
                "INSERT INTO tl3_schema_migrations VALUES ($1, $2)",
                &[&((index + 1) as i32), sql],
            )
            .unwrap();
    }
    client.execute("INSERT INTO tl3_worker_progress (run_key, node_id, epoch, attempt, output_offset, unit, started_ms, external_wait) VALUES ('existing', '1', 1, 1, 0, 'tokens', 1, $1)", &[&json!({"upstream": "prismabuild", "key": "existing-action"})]).unwrap();
    assert_eq!(
        client
            .query_one("SELECT MAX(version) FROM tl3_schema_migrations", &[])
            .unwrap()
            .get::<_, i32>(0),
        6
    );
    let upgraded = migrate(settings);
    assert!(
        upgraded.is_ok(),
        "schema 6 must upgrade to Hindsight: {:?}",
        upgraded.err()
    );
    for table in ["tl3_memory_outbox", "tl3_memory_health"] {
        assert!(
            client
                .query_one("SELECT to_regclass($1)::text IS NOT NULL", &[&table])
                .unwrap()
                .get::<_, bool>(0),
            "missing Hindsight table {table}"
        );
    }
    assert_eq!(
        client
            .query_one(
                "SELECT external_wait FROM tl3_worker_progress WHERE run_key = 'existing'",
                &[]
            )
            .unwrap()
            .get::<_, serde_json::Value>(0),
        json!({"upstream": "prismabuild", "key": "existing-action"})
    );
    assert_eq!(
        client
            .query_one(
                "SELECT sql FROM tl3_schema_migrations WHERE version = 6",
                &[]
            )
            .unwrap()
            .get::<_, String>(0),
        deployed[5]
    );
    let mut store = Store::open(settings).unwrap();
    store
        .queue_memory("upgrade-record", &json!({"summary": "after MVP"}))
        .unwrap();
    store.park_memory("upgrade-record", 422).unwrap();
    assert!(store.pending_memories(0).unwrap().is_empty());
    assert_eq!(
        client
            .query_one(
                "SELECT http_status FROM tl3_memory_outbox WHERE operation_id = 'upgrade-record'",
                &[]
            )
            .unwrap()
            .get::<_, i32>(0),
        422
    );
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
            .query(
                "SELECT version FROM tl3_schema_migrations ORDER BY version",
                &[]
            )
            .unwrap()
            .iter()
            .map(|row| row.get::<_, i32>(0))
            .collect::<Vec<_>>(),
        [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 17, 18, 19, 20
        ]
    );
    for table in [
        "tl3_events",
        "tl3_credit",
        "tl3_code_grades",
        "tl3_phase_history",
    ] {
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
        Err(Error::SchemaVersion { actual: 999, .. })
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
    let unsafe_settings = unsafe_database.create_uninitialized("unsafe_durability");
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

fn read_only_reader(settings: &tl3_config::Store) {
    assert!(matches!(Reader::open(settings), Err(Error::NotInitialized)));
    assert!(
        raw(settings)
            .query_one("SELECT to_regclass('tl3_schema_migrations') IS NULL", &[])
            .unwrap()
            .get::<_, bool>(0)
    );
    let mut writer = migrate(settings).unwrap();
    let change = seed(&mut writer);
    writer.commit(&change).unwrap();
    let before = writer.load().unwrap();
    let schema = |client: &mut Client| {
        client
            .query(
                "SELECT version, sql FROM tl3_schema_migrations ORDER BY version",
                &[],
            )
            .unwrap()
            .into_iter()
            .map(|row| (row.get::<_, i32>(0), row.get::<_, String>(1)))
            .collect::<Vec<_>>()
    };
    let migrations_before = schema(&mut raw(settings));
    let mut reader = Reader::open(settings).unwrap();
    assert_eq!(reader.load().unwrap(), before);
    for sql in [
        "CREATE TABLE forbidden (id int)",
        "INSERT INTO tl3_world VALUES ('forbidden', '1', '{}', 1)",
        "UPDATE tl3_world SET value = '{}'",
        "DELETE FROM tl3_world",
    ] {
        let mut tx = reader.transaction().unwrap();
        assert_eq!(
            tx.query_one("SHOW transaction_read_only", &[])
                .unwrap()
                .get::<_, &str>(0),
            "on"
        );
        assert_eq!(
            tx.query_one("SHOW default_transaction_read_only", &[])
                .unwrap()
                .get::<_, &str>(0),
            "on"
        );
        assert_eq!(
            tx.query_one("SHOW transaction_isolation", &[])
                .unwrap()
                .get::<_, &str>(0),
            "repeatable read"
        );
        let error = tx.batch_execute(sql).unwrap_err();
        assert_eq!(error.code().unwrap().code(), "25006");
    }
    assert_eq!(writer.load().unwrap(), before);
    assert_eq!(schema(&mut raw(settings)), migrations_before);
}

fn reader_schema_versions(settings: &tl3_config::Store) {
    assert_eq!(
        Reader::open(settings).err().unwrap().to_string(),
        "database not initialized"
    );
    let mut client = raw(settings);
    assert!(
        client
            .query_one("SELECT to_regclass('tl3_schema_migrations') IS NULL", &[])
            .unwrap()
            .get::<_, bool>(0)
    );
    migrate(settings).unwrap();
    let migrations = client
        .query(
            "SELECT version, sql FROM tl3_schema_migrations ORDER BY version",
            &[],
        )
        .unwrap();
    let expected = migrations.last().unwrap().get::<_, i32>(0);
    let future = expected + 1;
    for version in [0, future] {
        client
            .execute("DELETE FROM tl3_schema_migrations", &[])
            .unwrap();
        client
            .execute(
                "INSERT INTO tl3_schema_migrations VALUES ($1, 'incompatible')",
                &[&version],
            )
            .unwrap();
        assert_eq!(
            Reader::open(settings).err().unwrap().to_string(),
            format!("schema version {version}, expected {expected}")
        );
    }
    client
        .execute("DELETE FROM tl3_schema_migrations", &[])
        .unwrap();
    for row in migrations {
        client
            .execute(
                "INSERT INTO tl3_schema_migrations VALUES ($1, $2)",
                &[&row.get::<_, i32>(0), &row.get::<_, String>(1)],
            )
            .unwrap();
    }
    let mut reader = Reader::open(settings).unwrap();
    client
        .execute(
            "UPDATE tl3_schema_migrations SET version = $1 WHERE version = $2",
            &[&future, &expected],
        )
        .unwrap();
    assert_eq!(
        reader.load().unwrap_err().to_string(),
        format!("schema version {future}, expected {expected}")
    );
    client
        .execute(
            "UPDATE tl3_schema_migrations SET version = $1 WHERE version = $2",
            &[&expected, &future],
        )
        .unwrap();
    client
        .batch_execute("DROP TABLE tl3_world CASCADE")
        .unwrap();
    assert_eq!(
        Reader::open(settings).err().unwrap().to_string(),
        "database not initialized"
    );
}

fn ingress_retirement(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    store
        .register_ingress_translators(&["first", "second"])
        .unwrap();
    let payload = b"{ \"zen\": \"raw bytes\" }";
    for n in 0..103 {
        assert!(
            store
                .receive_ingress("github-webhook", &format!("delivery-{n}"), "ping", payload)
                .unwrap()
        );
    }
    assert!(
        !store
            .receive_ingress("github-webhook", "delivery-0", "ping", payload)
            .unwrap()
    );
    let batch = store.pending_ingress().unwrap();
    assert_eq!(batch.len(), 100);
    assert_eq!(batch[0].payload, payload);
    let mut last = 0;
    for stored in batch {
        last = stored.sequence;
        store.consume_ingress(last).unwrap();
    }
    store.advance_translator("first", last).unwrap();
    assert_eq!(
        store.ingress_after(0).unwrap().len(),
        100,
        "second still needs these rows"
    );
    store.advance_translator("second", last).unwrap();
    assert_eq!(
        store.ingress_after(0).unwrap().len(),
        3,
        "last cursor retired the first batch"
    );
    for stored in store.pending_ingress().unwrap() {
        last = stored.sequence;
        store.advance_translator("first", last).unwrap();
        store.advance_translator("second", last).unwrap();
        assert!(
            !store.ingress_after(last - 1).unwrap().is_empty(),
            "intake has not consumed it yet"
        );
        store.consume_ingress(last).unwrap();
    }
    assert!(store.ingress_after(0).unwrap().is_empty());
    drop(store);
    let mut store = Store::open(settings).unwrap();
    store
        .register_ingress_translators(&["first", "second", "later"])
        .unwrap();
    assert_eq!(
        store.translator_cursor("later").unwrap(),
        last,
        "new consumers start at the durable intake position even after deletion"
    );
    store
        .receive_ingress("github-webhook", "recent", "ping", payload)
        .unwrap();
    let recent = store.pending_ingress().unwrap()[0].sequence;
    assert_eq!(store.ingress_after(last).unwrap().len(), 1);
    store.consume_ingress(recent).unwrap();
    store
        .register_ingress_translators(&["first", "second", "later", "latest"])
        .unwrap();
    assert_eq!(
        store.translator_cursor("latest").unwrap(),
        recent,
        "late consumers skip retained history too"
    );
    store.advance_translator("first", recent).unwrap();
    store.advance_translator("second", recent).unwrap();
    assert_eq!(store.ingress_after(0).unwrap().len(), 1);
    store.advance_translator("later", recent).unwrap();
    assert!(store.ingress_after(0).unwrap().is_empty());
    store
        .receive_ingress("github-webhook", "without-translators", "ping", payload)
        .unwrap();
    let sequence = store.pending_ingress().unwrap()[0].sequence;
    store.register_ingress_translators(&[]).unwrap();
    store.consume_ingress(sequence).unwrap();
    assert!(store.ingress_after(0).unwrap().is_empty());
}

fn declared_wait(store: &mut Store, key: &str) -> ExternalWait {
    let epoch = store.load().unwrap().epoch.get();
    store.claim_worker(key, "0", epoch, 1).unwrap();
    let wait = ExternalWait {
        upstream: "prismabuild".into(),
        key: "a".repeat(64),
    };
    store
        .record_worker_progress(key, 1, 10, "external_wait", Some(&wait))
        .unwrap();
    assert_eq!(
        store
            .worker_progress(None)
            .unwrap()
            .iter()
            .find(|r| r.key == key)
            .unwrap()
            .external_wait
            .as_ref(),
        Some(&wait)
    );
    wait
}
fn wait_clears_on_progress(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    declared_wait(&mut store, "run:1");
    store
        .record_worker_progress("run:1", 1, 20, "tool_result", None)
        .unwrap();
    let row = store.worker_progress(None).unwrap().remove(0);
    assert!(row.external_wait.is_none(), "own progress clears a wait");
    assert_eq!(row.unit, "tool_result");
    assert!(row.last_progress_ms.is_some());
}
fn wait_resets_on_attempt(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    declared_wait(&mut store, "run:1");
    let epoch = store.load().unwrap().epoch.get();
    store.claim_worker("run:1", "0", epoch, 2).unwrap();
    let row = store.worker_progress(None).unwrap().remove(0);
    assert!(row.external_wait.is_none(), "new attempt clears a wait");
    assert_eq!(row.output_offset, 0);
    assert_eq!(row.attempt, 2);
    store
        .record_worker_progress(
            "run:1",
            1,
            30,
            "external_wait",
            Some(&ExternalWait {
                upstream: "prismabuild".into(),
                key: "b".repeat(64),
            }),
        )
        .unwrap();
    assert!(
        store
            .worker_progress(None)
            .unwrap()
            .remove(0)
            .external_wait
            .is_none(),
        "old attempt cannot restore its wait"
    );
}
fn wait_resets_on_generation(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    declared_wait(&mut store, "effect:0:1");
    let epoch = store.load().unwrap().epoch.get();
    store.claim_worker("effect:0:2", "0", epoch, 1).unwrap();
    assert!(
        store
            .worker_progress(None)
            .unwrap()
            .iter()
            .find(|r| r.key == "effect:0:2")
            .unwrap()
            .external_wait
            .is_none(),
        "new generation has no inherited wait"
    );
}

fn change_feed(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let mut listener = Listener::open(settings).unwrap();
    let change = seed(&mut store);
    store.commit(&change).unwrap();
    assert!(
        listener.wait().unwrap().contains("tl3_meta"),
        "a commit notifies"
    );
    let epoch = store.load().unwrap().epoch.get();
    store.claim_worker("run:1", "1", epoch, 1).unwrap();
    assert_eq!(
        listener.wait().unwrap(),
        ["tl3_worker_progress".to_string()].into(),
        "a new worker run notifies"
    );
    store
        .record_worker_progress("run:1", 1, 5, "tool_result", None)
        .unwrap();
    assert_eq!(
        listener.wait().unwrap(),
        ["tl3_worker_progress".to_string()].into(),
        "worker progress notifies"
    );
    // Notifications queue on the session until the listener takes them.
    for index in 0..3 {
        let snapshot = store.load().unwrap();
        let mut next = Commit::new(format!("event-{}", index + 2), json!(index), &snapshot);
        next.rows = vec![put("2", json!([index]))];
        store.commit(&next).unwrap();
    }
    assert!(listener.wait().unwrap().contains("tl3_meta"));
}

fn phase_entry_times(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let change = seed(&mut store);
    store.commit(&change).unwrap();
    let since = |client: &mut Client| {
        client
            .query_one(
                "SELECT phases.phase, phases.since = events.committed_at, events.revision
                 FROM tl3_node_phases phases JOIN tl3_events events ON events.committed_at = phases.since
                 WHERE phases.node_id = '1'",
                &[],
            )
            .map(|row| (row.get::<_, serde_json::Value>(0), row.get::<_, bool>(1), row.get::<_, i64>(2)))
            .unwrap()
    };
    let mut client = raw(settings);
    assert_eq!(since(&mut client), (json!("Making"), true, 1));
    let mut commit = |key: &str, value: serde_json::Value| {
        let snapshot = store.load().unwrap();
        let mut change = Commit::new(key, json!(key), &snapshot);
        change.rows = vec![put("1", value)];
        store.commit(&change).unwrap();
    };
    commit("same-phase", json!({"phase": "Making", "attempt": 2}));
    assert_eq!(
        since(&mut client),
        (json!("Making"), true, 1),
        "same phase keeps its entry time"
    );
    commit("next-phase", json!({"phase": {"Closed": "Merged"}}));
    assert_eq!(since(&mut client), (json!({"Closed": "Merged"}), true, 3));
    let snapshot = store.load().unwrap();
    let mut change = Commit::new("removed", json!("removed"), &snapshot);
    change.rows = vec![RowChange::Delete {
        table: "nodes".into(),
        id: "1".into(),
    }];
    store.commit(&change).unwrap();
    assert_eq!(
        client
            .query_one(
                "SELECT count(*) FROM tl3_node_phases WHERE node_id = '1'",
                &[]
            )
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

/// A version-9 world, from before the change feed, gains entry times from
/// each node's last change. Rows without a phase are skipped.
fn phase_backfill_from_schema_9(settings: &tl3_config::Store) {
    let mut client = raw(settings);
    client
        .batch_execute(
            "CREATE TABLE tl3_schema_migrations (version integer PRIMARY KEY, sql text NOT NULL)",
        )
        .unwrap();
    let schema_9 = [
        include_str!("../migrations/0001_store.sql"),
        include_str!("../migrations/0002_world_arrival.sql"),
        include_str!("../migrations/0003_workers.sql"),
        include_str!("../migrations/0004_ingress.sql"),
        include_str!("../migrations/0005_worker_retirement.sql"),
        include_str!("../migrations/0006_worker_wait.sql"),
        include_str!("../migrations/0007_hindsight.sql"),
        include_str!("../migrations/0008_memory_parking.sql"),
        include_str!("../migrations/0009_memory_recovery.sql"),
    ];
    for (index, sql) in schema_9.iter().enumerate() {
        client.batch_execute(sql).unwrap();
        client
            .execute(
                "INSERT INTO tl3_schema_migrations VALUES ($1, $2)",
                &[&((index + 1) as i32), sql],
            )
            .unwrap();
    }
    client
        .batch_execute(
            "INSERT INTO tl3_events (event_key, revision, epoch, payload, result, committed_at) VALUES
                 ('first', 1, 0, '1', '1', '2026-10-01 10:00:00+00'),
                 ('second', 2, 0, '2', '2', '2026-10-02 11:30:00+00');
             INSERT INTO tl3_world (table_name, row_id, value, version) VALUES
                 ('nodes', '0', '{\"phase\": {\"Closed\": \"Merged\"}}', 1),
                 ('nodes', '1', '{\"phase\": \"Making\", \"attempt\": 2}', 2),
                 ('nodes', '2', '[1, 2]', 2),
                 ('nodes', '3', '{\"attempt\": 1}', 1),
                 ('requests', '1', '{\"phase\": \"Making\"}', 2);
             UPDATE tl3_meta SET revision = 2;",
        )
        .unwrap();
    drop(migrate(settings).unwrap());
    let rows: Vec<(String, serde_json::Value, String)> = client
        .query(
            "SELECT node_id, phase, to_char(since AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI') FROM tl3_node_phases ORDER BY node_id",
            &[],
        )
        .unwrap()
        .into_iter()
        .map(|row| (row.get(0), row.get(1), row.get(2)))
        .collect();
    assert_eq!(
        rows,
        [
            (
                "0".into(),
                json!({"Closed": "Merged"}),
                "2026-10-01 10:00".into()
            ),
            ("1".into(), json!("Making"), "2026-10-02 11:30".into()),
        ]
    );
}

/// The console's login reads every table, including ones later migrations
/// add, and can't write. Granting twice changes nothing.
fn reader_role(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let change = seed(&mut store);
    store.commit(&change).unwrap();
    let password = settings.password_file.as_deref().unwrap();
    grant_reader(settings, "tl3_reader_test", password).unwrap();
    grant_reader(settings, "tl3_reader_test", password).unwrap();
    let reader = tl3_config::Store {
        user: "tl3_reader_test".into(),
        ..settings.clone()
    };
    assert_eq!(
        Reader::open(&reader).unwrap().load().unwrap(),
        store.load().unwrap()
    );
    Listener::open(&reader).unwrap();
    let mut owner = raw(settings);
    owner
        .batch_execute(
            "CREATE TABLE tl3_later_migration (id int); INSERT INTO tl3_later_migration VALUES (7)",
        )
        .unwrap();
    let mut client = postgres::Config::new()
        .host(&reader.host)
        .port(reader.port)
        .user(&reader.user)
        .dbname(&reader.database)
        .password("tl3-disposable-test-password")
        .connect(NoTls)
        .unwrap();
    assert_eq!(
        client
            .query_one("SELECT id FROM tl3_later_migration", &[])
            .unwrap()
            .get::<_, i32>(0),
        7
    );
    for sql in [
        "INSERT INTO tl3_world VALUES ('forbidden', '1', '{}', 1)",
        "UPDATE tl3_meta SET revision = revision",
        "DELETE FROM tl3_later_migration",
        "CREATE TABLE tl3_forbidden (id int)",
    ] {
        assert!(client.batch_execute(sql).is_err(), "{sql}");
    }
    let flags = owner
        .query_one(
            "SELECT rolsuper, rolcreatedb, rolcreaterole, rolreplication, rolcanlogin FROM pg_roles WHERE rolname = 'tl3_reader_test'",
            &[],
        )
        .unwrap();
    assert_eq!(
        (0..5)
            .map(|index| flags.get::<_, bool>(index))
            .collect::<Vec<_>>(),
        [false, false, false, false, true]
    );
    // A session that forgets the reader's read-only option still can't write.
    assert_eq!(
        owner
            .query_one(
                "SELECT rolconfig FROM pg_roles WHERE rolname = 'tl3_reader_test'",
                &[],
            )
            .unwrap()
            .get::<_, Option<Vec<String>>>(0),
        Some(vec!["default_transaction_read_only=on".to_string()])
    );
    assert!(grant_reader(settings, "bad role; DROP TABLE tl3_world", password).is_err());
}

/// PRD #327, C3: the console's login files a typed command through one
/// function, which wakes tl3d. It still can't write any table itself, and
/// tl3d's commit for the command settles it.
fn console_commands(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let change = seed(&mut store);
    store.commit(&change).unwrap();
    let password = settings.password_file.as_deref().unwrap();
    grant_reader(settings, "tl3_console_test", password).unwrap();
    let console = tl3_config::Store {
        user: "tl3_console_test".into(),
        ..settings.clone()
    };
    let mut wake = Listener::ingress(settings).unwrap();
    let mut changes = Listener::open(&console).unwrap();
    let mut commands = ConsoleCommands::open(&console).unwrap();
    let id = "0b9f3c3e-5f43-4a8e-9d1c-2f0e7c1d9a11";
    let action = json!({"kind": "pause"});
    assert!(commands.submit(id, 1, &action).unwrap());
    assert_eq!(wake.wait().unwrap(), ["console".to_string()].into());
    assert!(changes.wait().unwrap().contains("tl3_console_commands"));
    // Resubmitting the same id files nothing more.
    assert!(!commands.submit(id, 1, &action).unwrap());
    let pending = store.pending_ingress().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        (pending[0].source.as_str(), pending[0].delivery_key.as_str()),
        ("console", id)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&pending[0].payload).unwrap(),
        json!({"id": id, "node": 1, "action": action})
    );
    for (bad_id, node, action) in [
        ("not-a-uuid", 1, json!({"kind": "pause"})),
        (
            "0B9F3C3E-5F43-4A8E-9D1C-2F0E7C1D9A11",
            1,
            json!({"kind": "pause"}),
        ),
        ("1b9f3c3e-5f43-4a8e-9d1c-2f0e7c1d9a11", 1, json!("pause")),
    ] {
        assert!(
            commands.submit(bad_id, node, &action).is_err(),
            "{bad_id} {action}"
        );
    }
    let mut client = postgres::Config::new()
        .host(&console.host)
        .port(console.port)
        .user(&console.user)
        .dbname(&console.database)
        .password("tl3-disposable-test-password")
        .connect(NoTls)
        .unwrap();
    // The login can't write any table itself: each attempt is a privilege error.
    for sql in [
        "INSERT INTO tl3_ingress (source, delivery_key, event_name, payload) VALUES ('github-webhook', 'x', 'issues', '')",
        "INSERT INTO tl3_console_commands (id, node_id, action) VALUES ('x', 1, '{}')",
        "UPDATE tl3_console_commands SET status = 'applied'",
        "UPDATE tl3_meta SET revision = revision",
        "INSERT INTO tl3_world VALUES ('forbidden', '1', '{}', 1)",
        "INSERT INTO tl3_events (event_key, revision, epoch, payload, result) VALUES ('console:x', 99, 0, '{}', '{}')",
    ] {
        client.batch_execute("BEGIN READ WRITE").unwrap();
        let error = client.batch_execute(sql).unwrap_err();
        assert_eq!(
            error.code(),
            Some(&postgres::error::SqlState::INSUFFICIENT_PRIVILEGE),
            "{sql}: {error}"
        );
        client.batch_execute("ROLLBACK").unwrap();
    }
    // Only the console login and the owner may file commands.
    let mut other = raw(settings);
    other
        .batch_execute("CREATE ROLE tl3_other_test LOGIN PASSWORD 'tl3-disposable-test-password'")
        .unwrap_or(());
    let mut stranger = postgres::Config::new()
        .host(&console.host)
        .port(console.port)
        .user("tl3_other_test")
        .dbname(&console.database)
        .password("tl3-disposable-test-password")
        .connect(NoTls)
        .unwrap();
    let error = stranger
        .query_one(
            "SELECT tl3_console_command('2b9f3c3e-5f43-4a8e-9d1c-2f0e7c1d9a11', 1, '{\"kind\":\"stop\"}')",
            &[],
        )
        .unwrap_err();
    assert_eq!(
        error.code(),
        Some(&postgres::error::SqlState::INSUFFICIENT_PRIVILEGE),
        "{error}"
    );
    let status = |client: &mut Client| {
        client
            .query_one(
                "SELECT status, reason, revision FROM tl3_console_commands WHERE id = $1",
                &[&id],
            )
            .map(|row| {
                (
                    row.get::<_, String>(0),
                    row.get::<_, Option<String>>(1),
                    row.get::<_, Option<i64>>(2),
                )
            })
            .unwrap()
    };
    assert_eq!(status(&mut client), ("pending".into(), None, None));
    let snapshot = store.load().unwrap();
    let mut settle = Commit::new(format!("console:{id}"), action.clone(), &snapshot);
    settle.result =
        json!({"console": {"status": "refused", "reason": "The request is already paused."}});
    let receipt = store.commit(&settle).unwrap();
    assert_eq!(
        status(&mut client),
        (
            "refused".into(),
            Some("The request is already paused.".into()),
            Some(receipt.revision.get())
        )
    );
    // An unrelated commit leaves it alone.
    let snapshot = store.load().unwrap();
    store
        .commit(&Commit::new("console-unrelated", json!(1), &snapshot))
        .unwrap();
    assert_eq!(status(&mut client).0, "refused");
    console_command_caps(settings, &mut commands, &mut store);
}

/// Nothing oversized reaches ingress, the command table or the event log,
/// and a backlog tl3d isn't draining stops growing. Each refusal has its own
/// SQLSTATE so the console can say which.
fn console_command_caps(
    settings: &tl3_config::Store,
    commands: &mut ConsoleCommands,
    store: &mut Store,
) {
    let ingress = |store: &mut Store| store.pending_ingress().unwrap().len();
    let before = ingress(store);
    let code = |error: Error| match error {
        Error::Database(error) => error.code().cloned(),
        other => panic!("{other}"),
    };
    let big = "3b9f3c3e-5f43-4a8e-9d1c-2f0e7c1d9a11";
    let fits = "3b9f3c3e-5f43-4a8e-9d1c-2f0e7c1d9a12";
    // The cap is on the action's jsonb text, as the function measures it.
    let comment = |chars: usize| json!({"kind": "comment", "text": "a".repeat(chars)});
    let overhead: i32 = raw(settings)
        .query_one("SELECT octet_length($1::jsonb::text)", &[&comment(0)])
        .unwrap()
        .get(0);
    let at = comment(65_536 - overhead as usize);
    let over = comment(65_537 - overhead as usize);
    assert_eq!(
        code(commands.submit(big, 1, &over).unwrap_err()),
        Some(postgres::error::SqlState::STRING_DATA_RIGHT_TRUNCATION)
    );
    assert!(commands.submit(fits, 1, &at).unwrap());
    assert_eq!(
        ingress(store),
        before + 1,
        "only the command that fits is filed"
    );
    // tl3d isn't draining: at most 1,000 commands wait unsettled.
    raw(settings)
        .batch_execute(
            "INSERT INTO tl3_console_commands (id, node_id, action)
             SELECT 'backlog-' || n, 1, '{\"kind\":\"pause\"}' FROM generate_series(1, 1000 - (SELECT count(*) FROM tl3_console_commands WHERE status = 'pending')::int) n",
        )
        .unwrap();
    // A resend files nothing and isn't refused, even with the backlog full or
    // an oversized action under the same id.
    assert!(!commands.submit(fits, 1, &at).unwrap());
    assert!(!commands.submit(fits, 1, &over).unwrap());
    let late = "3b9f3c3e-5f43-4a8e-9d1c-2f0e7c1d9a13";
    let error = commands
        .submit(late, 1, &json!({"kind": "pause"}))
        .unwrap_err();
    let message = error.to_string();
    assert_eq!(
        code(error),
        Some(postgres::error::SqlState::CONFIGURATION_LIMIT_EXCEEDED),
        "{message}"
    );
    raw(settings)
        .batch_execute("UPDATE tl3_console_commands SET status = 'applied' WHERE id = 'backlog-1'")
        .unwrap();
    assert!(commands.submit(late, 1, &json!({"kind": "pause"})).unwrap());
}

const BUILTIN: &[(&str, &str)] = &[
    ("ingest", "Ingest, as shipped."),
    ("reviewer", "Review, as shipped."),
];

fn prompt_seeding(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    store.seed_prompts(BUILTIN).unwrap();
    store.seed_prompts(BUILTIN).unwrap();
    let mut prompts = Prompts::open(settings).unwrap();
    let roles = prompts.roles().unwrap();
    assert_eq!(
        roles.iter().map(|r| r.role.as_str()).collect::<Vec<_>>(),
        ["ingest", "reviewer"]
    );
    for (role, (name, body)) in roles.iter().zip(BUILTIN) {
        assert_eq!(&role.role, name);
        assert_eq!(role.versions.len(), 1, "a repeated seed adds nothing");
        let version = &role.versions[0];
        assert_eq!(
            (
                version.id,
                version.number,
                version.builtin,
                version.author.as_str()
            ),
            (role.active.id, 1, true, BUILTIN_AUTHOR)
        );
        assert_eq!(&role.active.body, body);
        assert!(!version.note.is_empty());
        assert_eq!(role.builtin_update, None);
    }
    assert_eq!(prompts.role("planner").unwrap(), None);
}

/// A deploy's changed built-in becomes active only where the previous built-in
/// was; elsewhere it waits as an update until Rob decides.
fn prompt_builtin_updates(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    store.seed_prompts(BUILTIN).unwrap();
    let mut prompts = Prompts::open(settings).unwrap();
    let base = prompts.role("ingest").unwrap().unwrap().active.id;
    let edited = prompts
        .save(
            "ingest",
            "Rob's ingest.",
            "Ask for smaller briefs.",
            "Rob",
            base,
        )
        .unwrap()
        .unwrap()
        .active
        .id;
    let shipped = [
        ("ingest", "Ingest, second release."),
        ("reviewer", "Review, second release."),
    ];
    store.seed_prompts(&shipped).unwrap();
    store.seed_prompts(&shipped).unwrap();
    let reviewer = prompts.role("reviewer").unwrap().unwrap();
    assert_eq!(reviewer.active.body, "Review, second release.");
    assert_eq!(reviewer.versions.len(), 2);
    assert_eq!(reviewer.builtin_update, None);
    let ingest = prompts.role("ingest").unwrap().unwrap();
    assert_eq!(ingest.active.id, edited, "Rob's version stays active");
    assert_eq!(ingest.versions.len(), 3);
    let update = ingest.builtin_update.expect("the console flags the update");
    assert_eq!(
        (update.body.as_str(), update.builtin, update.number),
        ("Ingest, second release.", true, 3)
    );
    // Keeping Rob's version settles this update, not the next one.
    assert_eq!(
        prompts.keep("ingest", update.id + 100).unwrap(),
        Err(Refusal::NoUpdate)
    );
    let kept = prompts.keep("ingest", update.id).unwrap().unwrap();
    assert_eq!((kept.active.id, kept.builtin_update), (edited, None));
    store
        .seed_prompts(&[("ingest", "Ingest, third release.")])
        .unwrap();
    let ingest = prompts.role("ingest").unwrap().unwrap();
    assert_eq!(ingest.active.id, edited);
    let third = ingest.builtin_update.unwrap().id;
    // Choosing the built-in again lets later deploys update it.
    let chosen = prompts
        .activate("ingest", third, "Rob", edited)
        .unwrap()
        .unwrap();
    assert_eq!((chosen.active.id, chosen.builtin_update), (third, None));
    store
        .seed_prompts(&[("ingest", "Ingest, fourth release.")])
        .unwrap();
    let ingest = prompts.role("ingest").unwrap().unwrap();
    assert_eq!(ingest.active.body, "Ingest, fourth release.");
    assert_eq!(ingest.builtin_update, None);
    assert_eq!(ingest.activated_by, BUILTIN_AUTHOR);
    // Reverting to an older built-in is Rob's choice: no update to flag, and
    // the next deploy leaves it active.
    let reverted = prompts
        .activate("ingest", third, "Rob", ingest.active.id)
        .unwrap()
        .unwrap();
    assert_eq!((reverted.active.id, reverted.builtin_update), (third, None));
    store
        .seed_prompts(&[("ingest", "Ingest, fifth release.")])
        .unwrap();
    let ingest = prompts.role("ingest").unwrap().unwrap();
    assert_eq!(ingest.active.id, third);
    assert_eq!(
        ingest.builtin_update.unwrap().body,
        "Ingest, fifth release."
    );
}

fn prompt_edits_and_revert(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    store.seed_prompts(BUILTIN).unwrap();
    let mut prompts = Prompts::open(settings).unwrap();
    let first = prompts.role("reviewer").unwrap().unwrap().active.id;
    assert_eq!(
        prompts
            .save("reviewer", "Text", "  ", "Rob", first)
            .unwrap(),
        Err(Refusal::EmptyNote)
    );
    assert_eq!(
        prompts
            .save("reviewer", " \n", "Note", "Rob", first)
            .unwrap(),
        Err(Refusal::EmptyText)
    );
    assert_eq!(
        prompts
            .save("planner", "Text", "Note", "Rob", first)
            .unwrap(),
        Err(Refusal::UnknownRole)
    );
    let saved = prompts
        .save(
            "reviewer",
            "Review v2.",
            " Check the tests first. ",
            "Rob",
            first,
        )
        .unwrap()
        .unwrap();
    let second = &saved.active;
    assert_eq!(saved.versions[0].id, second.id);
    assert_eq!(
        (
            second.number,
            second.body.as_str(),
            second.note.as_str(),
            second.author.as_str(),
            second.builtin
        ),
        (2, "Review v2.", "Check the tests first.", "Rob", false)
    );
    assert_eq!(saved.activated_by, "Rob");
    assert!(saved.versions[0].created_ms >= saved.versions[1].created_ms);
    // An editor still showing the first version can't overwrite the second.
    assert_eq!(
        prompts
            .save("reviewer", "Review v2b.", "Note", "Rob", first)
            .unwrap(),
        Err(Refusal::Stale { active: second.id })
    );
    let reverted = prompts
        .activate("reviewer", first, "Rob", second.id)
        .unwrap()
        .unwrap();
    assert_eq!(reverted.active.id, first);
    assert_eq!(reverted.active.body, "Review, as shipped.");
    assert_eq!(reverted.versions.len(), 2, "a revert adds no version");
    assert_eq!(reverted.builtin_update, None);
    let ingest_version = prompts.role("ingest").unwrap().unwrap().active.id;
    assert_eq!(
        prompts
            .activate("reviewer", ingest_version, "Rob", first)
            .unwrap(),
        Err(Refusal::UnknownVersion),
        "a version of another role"
    );
    assert_eq!(
        prompts.activate("reviewer", 9_999, "Rob", first).unwrap(),
        Err(Refusal::UnknownVersion)
    );
}

/// A revert from a page that missed a newer save changes nothing.
fn prompt_activation_needs_the_active_version(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    store.seed_prompts(BUILTIN).unwrap();
    let mut prompts = Prompts::open(settings).unwrap();
    let first = prompts.role("reviewer").unwrap().unwrap().active.id;
    let second = prompts
        .save("reviewer", "Review v2.", "Second.", "Rob", first)
        .unwrap()
        .unwrap()
        .active
        .id;
    let third = prompts
        .save("reviewer", "Review v3.", "Third.", "Rob", second)
        .unwrap()
        .unwrap()
        .active
        .id;
    // A page that loaded while v2 was active tries to revert to v1.
    assert_eq!(
        prompts.activate("reviewer", first, "Rob", second).unwrap(),
        Err(Refusal::Stale { active: third })
    );
    assert_eq!(
        prompts.role("reviewer").unwrap().unwrap().active.id,
        third,
        "the newer save stays active"
    );
}

/// History rows carry no text; one version's text comes on request; text and
/// notes have limits.
fn prompt_history_and_limits(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    store.seed_prompts(BUILTIN).unwrap();
    let mut prompts = Prompts::open(settings).unwrap();
    let first = prompts.role("reviewer").unwrap().unwrap().active.id;
    let long_note = "n".repeat(NOTE_LIMIT + 1);
    assert_eq!(
        prompts
            .save("reviewer", "Text", &long_note, "Rob", first)
            .unwrap(),
        Err(Refusal::NoteTooLong)
    );
    let long_text = "t".repeat(TEXT_LIMIT + 1);
    assert_eq!(
        prompts
            .save("reviewer", &long_text, "Note", "Rob", first)
            .unwrap(),
        Err(Refusal::TextTooLong)
    );
    let largest = "é".repeat(TEXT_LIMIT / 2);
    let saved = prompts
        .save("reviewer", &largest, &"n".repeat(NOTE_LIMIT), "Rob", first)
        .unwrap()
        .unwrap();
    assert_eq!(saved.active.body, largest, "the limits are inclusive");
    assert_eq!(
        saved.versions.iter().map(|v| v.number).collect::<Vec<_>>(),
        [2, 1]
    );
    let shipped = prompts.version("reviewer", first).unwrap().unwrap();
    assert_eq!(
        (shipped.body.as_str(), shipped.number, shipped.builtin),
        ("Review, as shipped.", 1, true)
    );
    assert_eq!(prompts.version("ingest", first).unwrap(), None);
    assert_eq!(prompts.version("reviewer", 9_999).unwrap(), None);
    // The table holds the same limits for any other writer.
    let mut owner = raw(settings);
    for (body, note) in [(long_text.as_str(), "Note"), ("Text", long_note.as_str())] {
        assert!(
            owner
                .execute(
                    "INSERT INTO tl3_prompt_versions (role, body, author, note) VALUES ('reviewer', $1, 'Rob', $2)",
                    &[&body, &note],
                )
                .is_err()
        );
    }
}

/// A session Postgres has dropped reports itself dead before an edit is sent.
fn prompt_session_liveness(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    store.seed_prompts(BUILTIN).unwrap();
    let (login, _) = console_login(settings, "tl3_console_liveness");
    let mut prompts = Prompts::open(&login).unwrap();
    assert!(prompts.is_live());
    raw(settings)
        .execute(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename = 'tl3_console_liveness'",
            &[],
        )
        .unwrap();
    assert!(!prompts.is_live());
    assert!(Prompts::open(&login).unwrap().is_live());
}

/// The console judges migrations itself and serves a store ahead of the TL3 it
/// pins, so the prompt session needs only the prompt tables.
fn prompts_open_on_a_store_ahead(settings: &tl3_config::Store) {
    assert!(
        Prompts::open(settings).is_err(),
        "a store without the prompt tables"
    );
    let mut store = migrate(settings).unwrap();
    store.seed_prompts(BUILTIN).unwrap();
    raw(settings)
        .execute(
            "INSERT INTO tl3_schema_migrations VALUES (999, 'SELECT 1')",
            &[],
        )
        .unwrap();
    assert!(matches!(
        Reader::open(settings),
        Err(Error::SchemaVersion { .. })
    ));
    let mut prompts = Prompts::open(settings).unwrap();
    let base = prompts.role("ingest").unwrap().unwrap().active.id;
    prompts
        .save("ingest", "Ingest v2.", "Ahead.", "Rob", base)
        .unwrap()
        .unwrap();
}

/// A run keeps the version it started with; later runs take the active one.
fn prompt_version_recorded_for_a_run(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    assert_eq!(store.run_prompt("effect:0:1", "ingest").unwrap(), None);
    store.seed_prompts(BUILTIN).unwrap();
    let first = store.run_prompt("effect:0:1", "ingest").unwrap().unwrap();
    assert_eq!(
        (first.body.as_str(), first.role.as_str()),
        ("Ingest, as shipped.", "ingest")
    );
    let mut prompts = Prompts::open(settings).unwrap();
    let saved = prompts
        .save("ingest", "Ingest v2.", "Shorter.", "Rob", first.id)
        .unwrap()
        .unwrap()
        .active
        .id;
    assert_eq!(
        store.run_prompt("effect:0:1", "ingest").unwrap().unwrap(),
        first,
        "a retry of the same run keeps its version"
    );
    assert_eq!(
        store
            .run_prompt("effect:0:2", "ingest")
            .unwrap()
            .unwrap()
            .id,
        saved
    );
    let recorded: Vec<(String, String, i64)> = raw(settings)
        .query(
            "SELECT run_key, role, version FROM tl3_prompt_runs ORDER BY run_key",
            &[],
        )
        .unwrap()
        .iter()
        .map(|row| (row.get(0), row.get(1), row.get(2)))
        .collect();
    assert_eq!(
        recorded,
        [
            ("effect:0:1".into(), "ingest".into(), first.id),
            ("effect:0:2".into(), "ingest".into(), saved),
        ]
    );
}

fn console_login(settings: &tl3_config::Store, role: &str) -> (tl3_config::Store, Client) {
    grant_reader(settings, role, settings.password_file.as_deref().unwrap()).unwrap();
    let login = tl3_config::Store {
        user: role.into(),
        ..settings.clone()
    };
    let client = raw(&login);
    (login, client)
}

/// The console's login edits prompts through `Prompts` and can write nothing else.
fn console_login_writes_only_prompts(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let change = seed(&mut store);
    store.commit(&change).unwrap();
    store.seed_prompts(BUILTIN).unwrap();
    let run = store.run_prompt("effect:0:1", "ingest").unwrap().unwrap();
    let (login, mut client) = console_login(settings, "tl3_console_prompts");
    let mut prompts = Prompts::open(&login).unwrap();
    let base = prompts.role("ingest").unwrap().unwrap().active.id;
    let saved = prompts
        .save("ingest", "Rob's ingest.", "Edited.", "Rob", base)
        .unwrap()
        .unwrap()
        .active
        .id;
    // Reverting leaves the saved version referenced by nothing.
    prompts
        .activate("ingest", base, "Rob", saved)
        .unwrap()
        .unwrap();
    let reviewer = prompts.role("reviewer").unwrap().unwrap().active.id;
    prompts
        .save("reviewer", "Rob's review.", "Edited.", "Rob", reviewer)
        .unwrap()
        .unwrap();
    store
        .seed_prompts(&[("reviewer", "Review, changed.")])
        .unwrap();
    let update = prompts
        .role("reviewer")
        .unwrap()
        .unwrap()
        .builtin_update
        .unwrap()
        .id;
    let kept = prompts.keep("reviewer", update).unwrap().unwrap();
    assert_eq!(kept.builtin_update, None);
    // A role with versions and no pointer, so a forged pointer breaks no constraint.
    let mut owner = raw(settings);
    let planner: i64 = owner
        .query_one(
            "INSERT INTO tl3_prompt_versions (role, body, author, note) VALUES ('planner', 'Plan.', 'TL3', 'Fixture.') RETURNING id",
            &[],
        )
        .unwrap()
        .get(0);
    // Each statement would succeed for an owner; only the login's grants refuse it.
    let statements = [
        "INSERT INTO tl3_world VALUES ('forbidden', '1', '{}', (SELECT max(revision) FROM tl3_events))".to_string(),
        "UPDATE tl3_meta SET revision = revision".into(),
        "INSERT INTO tl3_worker_materials VALUES ('9', '{}')".into(),
        "INSERT INTO tl3_prompt_versions (role, body, builtin, author, note) VALUES ('ingest', 'x', true, 'TL3 deploy', 'forged')".into(),
        format!("UPDATE tl3_prompt_versions SET body = 'rewritten' WHERE id = {saved}"),
        format!("DELETE FROM tl3_prompt_versions WHERE id = {saved}"),
        "DELETE FROM tl3_prompt_active WHERE role = 'reviewer'".into(),
        format!("INSERT INTO tl3_prompt_active (role, version, builtin_seen, activated_by) VALUES ('planner', {planner}, {planner}, 'Rob')"),
        format!("UPDATE tl3_prompt_active SET role = 'planner', version = {planner}, builtin_seen = {planner} WHERE role = 'ingest'"),
        format!("INSERT INTO tl3_prompt_runs (run_key, role, version) VALUES ('forged', 'ingest', {})", run.id),
        "UPDATE tl3_prompt_runs SET version = version".into(),
        "DELETE FROM tl3_prompt_runs".into(),
    ];
    for sql in &statements {
        let mut tx = owner.transaction().unwrap();
        tx.batch_execute(sql)
            .unwrap_or_else(|error| panic!("the owner can run {sql}: {error}"));
        tx.rollback().unwrap();
        let error = client
            .batch_execute(&format!("BEGIN READ WRITE; {sql}; COMMIT"))
            .expect_err(sql);
        assert_eq!(
            error.code(),
            Some(&postgres::error::SqlState::INSUFFICIENT_PRIVILEGE),
            "{sql}: {error}"
        );
        client.batch_execute("ROLLBACK").unwrap();
    }
    assert_eq!(
        owner
            .query_one("SELECT count(*) FROM tl3_prompt_runs", &[])
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    assert!(
        owner
            .query_one(
                "SELECT body FROM tl3_prompt_versions WHERE id = $1",
                &[&saved]
            )
            .unwrap()
            .get::<_, String>(0)
            == "Rob's ingest."
    );
}

/// Deploy may grant the console's login before tl3d migrates the store.
fn console_login_granted_before_migration(settings: &tl3_config::Store) {
    let (login, _) = console_login(settings, "tl3_console_early");
    let mut store = migrate(settings).unwrap();
    store.seed_prompts(BUILTIN).unwrap();
    assert_eq!(
        raw(&login)
            .query_one("SELECT count(*) FROM tl3_worker_runs", &[])
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    let mut prompts = Prompts::open(&login).unwrap();
    let base = prompts.role("reviewer").unwrap().unwrap().active.id;
    prompts
        .save("reviewer", "Review v2.", "Edited.", "Rob", base)
        .unwrap()
        .unwrap();
}

/// #403: owed postmortems and code grades commit with the fact that makes them.
fn postmortem_records(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let mut listener = Listener::open(settings).unwrap();
    assert_eq!(store.postmortem_sample_share().unwrap(), 0.1);
    let owed = |trigger: &str, reason: &str| OwedPostmortem {
        node: "1".into(),
        trigger: trigger.into(),
        reason: reason.into(),
        sample_share: None,
        repo: "owner/sandbox".into(),
        issue: 12,
        title: "Fix greeting".into(),
    };
    let grade = |dimension: &str, grade: Option<i16>| CodeGrade {
        node: "1".into(),
        trigger: "close".into(),
        rubric_version: 1,
        dimension: dimension.into(),
        grade,
        fact: json!({"attempts": 1}),
    };
    let mut change = seed(&mut store);
    change.postmortems = vec![owed("close", "needed_fix")];
    change.grades = vec![grade("reliability", Some(4)), grade("credit", None)];
    store.commit(&change).unwrap();
    store.commit(&change).unwrap();
    let tables = listener.wait().unwrap();
    assert!(tables.contains("tl3_postmortems"), "{tables:?}");
    let snapshot = store.load().unwrap();
    let mut again = Commit::new("again", json!("again"), &snapshot);
    again.postmortems = vec![owed("close", "retry"), owed("question:3", "escalation")];
    again.grades = vec![grade("reliability", Some(1))];
    store.commit(&again).unwrap();
    let records = store.postmortems("1").unwrap();
    assert_eq!(
        records
            .iter()
            .map(|r| (r.trigger.as_str(), r.reason.as_str(), r.status.as_str()))
            .collect::<Vec<_>>(),
        [
            ("close", "needed_fix", "owed"),
            ("question:3", "escalation", "owed")
        ],
        "each trigger is owed once, whatever replays or later commits say"
    );
    let grades = store.code_grades("1").unwrap();
    assert_eq!(
        grades,
        vec![grade("reliability", Some(4)), grade("credit", None)]
    );
    let mut client = raw(settings);
    assert!(
        client
            .query_one(
                "SELECT owed_at = committed_at FROM tl3_postmortems p JOIN tl3_events e ON e.event_key = 'event-1' WHERE p.trigger = 'close'",
                &[],
            )
            .unwrap()
            .get::<_, bool>(0),
        "owed with the commit that made it owed"
    );
    // Phase changes are kept as history from now on, for time per phase.
    let mut phase = |key: &str, value: serde_json::Value| {
        let snapshot = store.load().unwrap();
        let mut change = Commit::new(key, json!(key), &snapshot);
        change.rows = vec![put("1", value)];
        store.commit(&change).unwrap();
    };
    phase("same-phase", json!({"phase": "Making", "attempt": 2}));
    phase("closed", json!({"phase": {"Closed": "Merged"}}));
    let history: Vec<serde_json::Value> = client
        .query(
            "SELECT phase FROM tl3_phase_history WHERE node_id = '1' ORDER BY since, id",
            &[],
        )
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(history, [json!("Making"), json!({"Closed": "Merged"})]);
    let password = settings.password_file.as_deref().unwrap();
    grant_reader(settings, "tl3_postmortem_reader", password).unwrap();
    let mut reader = postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user("tl3_postmortem_reader")
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect(NoTls)
        .unwrap();
    for table in [
        "tl3_postmortems",
        "tl3_postmortem_recommendations",
        "tl3_code_grades",
        "tl3_phase_history",
        "tl3_settings",
    ] {
        reader
            .query(&format!("SELECT * FROM {table}"), &[])
            .unwrap_or_else(|error| panic!("the console reads {table}: {error}"));
    }
    assert!(
        client
            .execute(
                "UPDATE tl3_settings SET value = '1.5' WHERE key = 'postmortem_sample_share'",
                &[],
            )
            .is_err(),
        "the share stays between 0 and 1"
    );
}

/// #403: migration 13 on a populated database keeps tl3_node_phases exact and
/// starts the phase history from each node's current phase.
fn postmortem_migration_on_schema_12(settings: &tl3_config::Store) {
    let mut client = raw(settings);
    client
        .batch_execute(
            "CREATE TABLE tl3_schema_migrations (version integer PRIMARY KEY, sql text NOT NULL)",
        )
        .unwrap();
    let schema_12 = [
        include_str!("../migrations/0001_store.sql"),
        include_str!("../migrations/0002_world_arrival.sql"),
        include_str!("../migrations/0003_workers.sql"),
        include_str!("../migrations/0004_ingress.sql"),
        include_str!("../migrations/0005_worker_retirement.sql"),
        include_str!("../migrations/0006_worker_wait.sql"),
        include_str!("../migrations/0007_hindsight.sql"),
        include_str!("../migrations/0008_memory_parking.sql"),
        include_str!("../migrations/0009_memory_recovery.sql"),
        include_str!("../migrations/0010_change_feed.sql"),
        include_str!("../migrations/0011_console_commands.sql"),
        include_str!("../migrations/0012_prompts.sql"),
    ];
    for (index, sql) in schema_12.iter().enumerate() {
        client.batch_execute(sql).unwrap();
        client
            .execute(
                "INSERT INTO tl3_schema_migrations VALUES ($1, $2)",
                &[&((index + 1) as i32), sql],
            )
            .unwrap();
    }
    client
        .batch_execute(
            "INSERT INTO tl3_events (event_key, revision, epoch, payload, result, committed_at) VALUES
                 ('first', 1, 0, '1', '1', '2026-10-01 10:00:00+00'),
                 ('second', 2, 0, '2', '2', '2026-10-02 11:30:00+00');
             INSERT INTO tl3_world (table_name, row_id, value, version) VALUES
                 ('nodes', '0', '{\"phase\": {\"Closed\": \"Merged\"}}', 1),
                 ('nodes', '1', '{\"phase\": \"Making\", \"attempt\": 2}', 2);
             UPDATE tl3_meta SET revision = 2;",
        )
        .unwrap();
    let phases = |client: &mut Client, table: &str| -> Vec<(String, serde_json::Value, String)> {
        client
            .query(
                &format!("SELECT node_id, phase, to_char(since AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI') FROM {table} ORDER BY node_id, since"),
                &[],
            )
            .unwrap()
            .iter()
            .map(|row| (row.get(0), row.get(1), row.get(2)))
            .collect()
    };
    let before = phases(&mut client, "tl3_node_phases");
    assert_eq!(before.len(), 2);
    let mut store = migrate(settings).unwrap();
    assert_eq!(
        phases(&mut client, "tl3_node_phases"),
        before,
        "the trigger swap changes no entry time"
    );
    assert_eq!(
        phases(&mut client, "tl3_phase_history"),
        before,
        "history starts from each node's current phase"
    );
    let snapshot = store.load().unwrap();
    let mut change = Commit::new("same", json!("same"), &snapshot);
    change.rows = vec![put("1", json!({"phase": "Making", "attempt": 3}))];
    store.commit(&change).unwrap();
    assert_eq!(
        phases(&mut client, "tl3_node_phases"),
        before,
        "the same phase keeps its time"
    );
    let snapshot = store.load().unwrap();
    let mut change = Commit::new("next", json!("next"), &snapshot);
    change.rows = vec![put("1", json!({"phase": "Checking"}))];
    store.commit(&change).unwrap();
    let now = phases(&mut client, "tl3_node_phases");
    assert_eq!(now[0], before[0]);
    assert_eq!(now[1].1, json!("Checking"));
    let history = phases(&mut client, "tl3_phase_history");
    assert_eq!(history.len(), 3);
    assert_eq!(history[2], now[1]);
}

/// #403, #333: the console's login may change the sample share within its
/// bounds, and nothing else in the settings.
fn settings_editor(settings: &tl3_config::Store) {
    drop(Store::open(settings).unwrap());
    let password = settings.password_file.as_deref().unwrap();
    grant_reader(settings, "tl3_settings_test", password).unwrap();
    let mut reader = postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user("tl3_settings_test")
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect(NoTls)
        .unwrap();
    let update = |client: &mut Client, value: &str| {
        client.batch_execute(&format!(
            "BEGIN READ WRITE;
             UPDATE tl3_settings SET value = '{value}', updated_by = 'console', updated_at = now()
                 WHERE key = 'postmortem_sample_share';
             COMMIT;"
        ))
    };
    update(&mut reader, "0.25").unwrap();
    assert_eq!(
        Store::open(settings)
            .unwrap()
            .postmortem_sample_share()
            .unwrap(),
        0.25
    );
    for value in ["1.5", "-0.1", "\"half\""] {
        let error = update(&mut reader, value).unwrap_err();
        assert_eq!(
            error.code(),
            Some(&postgres::error::SqlState::CHECK_VIOLATION),
            "{value}: {error}"
        );
        reader.batch_execute("ROLLBACK").unwrap();
    }
    assert!(
        reader
            .batch_execute("BEGIN READ WRITE; UPDATE tl3_settings SET key = 'other'; COMMIT;")
            .is_err(),
        "the editor changes values only"
    );
    reader.batch_execute("ROLLBACK").unwrap();
    assert_eq!(
        Store::open(settings)
            .unwrap()
            .postmortem_sample_share()
            .unwrap(),
        0.25
    );
}

fn worker_completion_facts(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let facts = WorkerRunFacts {
        prismabuild: serde_json::from_value(json!([{
            "phase": "prismabuild_capability", "cause": "machine_record_unsupported",
            "fallback_used": true, "transport": "stdout", "runtime": "pbrun"
        }]))
        .unwrap(),
        started_ms: 1_000_000,
        finished_ms: Some(9_666_000),
        outcome: "succeeded".into(),
        release: Some("release-fixture".into()),
        model: "fixture-model".into(),
        input_tokens: Some(1_600_000),
        cached_tokens: Some(1_000_000),
        usage_complete: true,
        output_tokens: Some(170_000),
        trace_id: Some("tr-0f626041aebfeefcdea5b8365b1a3e46".into()),
        session_id: Some("tl3-83-17-1".into()),
        failure: None,
        retro: None,
    };
    store
        .record_worker_facts("run:83:17", "83", 0, "maker", &facts)
        .unwrap();
    store
        .record_worker_facts("run:83:17", "83", 0, "maker", &facts)
        .unwrap();
    let mut second = facts.clone();
    second.finished_ms = Some(second.started_ms + 764_000);
    second.outcome = "failed".into();
    second.cached_tokens = None;
    second.failure = Some(WorkerBoundaryFailure {
        origin: "proxy".into(),
        class: "InternalServerError".into(),
        status: Some(502),
        request_id: Some("req-fixture-120".into()),
    });
    store
        .record_worker_facts("run:83:17", "83", 1, "maker", &second)
        .unwrap();
    let mut client = raw(settings);
    let rows = client.query("SELECT attempt, finished_ms - started_ms, input_tokens, cached_tokens, output_tokens, failure_request_id FROM tl3_worker_runs ORDER BY attempt", &[]).unwrap();
    assert_eq!(rows.len(), 2, "replay must not duplicate attempts");
    assert_eq!(rows[0].get::<_, i64>(1), 8_666_000);
    assert_eq!(rows[1].get::<_, i64>(1), 764_000);
    assert_eq!(rows[1].get::<_, Option<i64>>(3), None);
    assert_eq!(rows[1].get::<_, String>(5), "req-fixture-120");
    assert!(
        store
            .record_worker_facts("run:83:17", "83", 0, "maker", &second)
            .is_err(),
        "a terminal record is immutable"
    );
    grant_reader(
        settings,
        "tl3_worker_facts_reader",
        settings.password_file.as_deref().unwrap(),
    )
    .unwrap();
    assert!(client.query_one("SELECT has_table_privilege('tl3_worker_facts_reader', 'tl3_worker_runs', 'SELECT') AND NOT has_table_privilege('tl3_worker_facts_reader', 'tl3_worker_runs', 'INSERT,UPDATE,DELETE')", &[]).unwrap().get::<_, bool>(0));
    let login = tl3_config::Store {
        user: "tl3_worker_facts_reader".into(),
        ..settings.clone()
    };
    assert_eq!(
        raw(&login)
            .query_one(
                "SELECT count(*) FROM tl3_worker_runs WHERE model = 'fixture-model'",
                &[]
            )
            .unwrap()
            .get::<_, i64>(0),
        2
    );
}

fn recommendation(rank: i16) -> Recommendation {
    Recommendation {
        fingerprint: format!("rec-{rank:016x}"),
        rank,
        phase: "maker".into(),
        kind: "prompt".into(),
        target: "maker".into(),
        operation: "add".into(),
        before: String::new(),
        after: format!("Run check {rank}."),
        evidence: "The review.".into(),
        effect: "Fewer fix rounds.".into(),
    }
}

fn judged(recommendations: i16) -> PostmortemResult {
    PostmortemResult {
        model: "claude-opus-5-5".into(),
        cause: "tl3".into(),
        summary: "The maker needed a fix round.".into(),
        timeline: json!([{"at": "Making", "event": "The maker ran."}]),
        process_failures: json!([{"phase": "maker", "kind": "code defect", "what": "x", "evidence": "y"}]),
        grades: json!([{"dimension": "correctness", "grade": 4}]),
        notes: "Cheaper with one check.".into(),
        assessment_ids: json!(["a-1", "a-2"]),
        recommendations: (1..=recommendations).map(recommendation).collect(),
        code_grades: vec![CodeGrade {
            node: "1".into(),
            trigger: "postmortem:1".into(),
            rubric_version: 1,
            dimension: "tokens".into(),
            grade: None,
            fact: json!({"input_tokens": 10}),
        }],
        memory: Some((
            "00000000-0000-5000-8000-000000000403".into(),
            json!({"content": "lesson", "tags": ["postmortem"]}),
        )),
    }
}

/// #403: the runner claims an owed postmortem, and only the claim's run key
/// can finish it. Results, recommendations, later grades and lessons commit
/// together; a stale or second claim changes nothing.
fn postmortem_results(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let owed = |trigger: &str| OwedPostmortem {
        node: "1".into(),
        trigger: trigger.into(),
        reason: "needed_fix".into(),
        sample_share: None,
        repo: "owner/sandbox".into(),
        issue: 12,
        title: "Fix greeting".into(),
    };
    let mut change = seed(&mut store);
    change.postmortems = vec![owed("close"), owed("question:3")];
    store.commit(&change).unwrap();
    let first = store.owed_postmortem().unwrap().unwrap();
    assert_eq!(
        (first.trigger.as_str(), first.status.as_str()),
        ("close", "owed")
    );
    assert!(store.running_postmortems().unwrap().is_empty());
    let mut listener = Listener::open(settings).unwrap();
    let update = |store: &mut Store, key: &str, update: PostmortemUpdate| {
        let snapshot = store.load().unwrap();
        let mut change = Commit::new(key, json!(key), &snapshot);
        change.rows = vec![RowChange::Put {
            table: "postmortem_jobs".into(),
            id: key.into(),
            value: json!(key),
        }];
        change.postmortem_updates = vec![update];
        store.commit(&change)
    };
    let claim = |id: i64, run_key: &str| PostmortemUpdate::Claim {
        id,
        run_key: run_key.into(),
        rubric_version: 1,
    };
    update(&mut store, "claim-1", claim(first.id, "postmortem:1:a")).unwrap();
    assert!(listener.wait().unwrap().contains("tl3_postmortems"));
    let running = store.running_postmortems().unwrap();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].run_key.as_deref(), Some("postmortem:1:a"));
    assert_eq!(
        store.owed_postmortem().unwrap().unwrap().trigger,
        "question:3"
    );
    // A second claim of a running postmortem fails its whole commit.
    assert!(update(&mut store, "claim-again", claim(first.id, "postmortem:1:b")).is_err());
    assert!(
        !store
            .load()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "postmortem_jobs" && row.id == "claim-again"),
        "the failed claim's rows rolled back"
    );
    // A result under another run key is stale and changes nothing.
    update(
        &mut store,
        "stale",
        PostmortemUpdate::Done {
            id: first.id,
            run_key: "postmortem:1:b".into(),
            result: Box::new(judged(3)),
        },
    )
    .unwrap();
    let detail = store.postmortem(first.id).unwrap().unwrap();
    assert_eq!(detail.record.status, "running");
    assert!(detail.recommendations.is_empty());
    assert!(store.code_grades("1").unwrap().is_empty());
    update(
        &mut store,
        "done",
        PostmortemUpdate::Done {
            id: first.id,
            run_key: "postmortem:1:a".into(),
            result: Box::new(judged(3)),
        },
    )
    .unwrap();
    let detail = store.postmortem(first.id).unwrap().unwrap();
    assert_eq!(detail.record.status, "done");
    assert_eq!(detail.cause.as_deref(), Some("tl3"));
    assert_eq!(
        detail.summary.as_deref(),
        Some("The maker needed a fix round.")
    );
    assert_eq!(detail.notes.as_deref(), Some("Cheaper with one check."));
    assert_eq!(
        detail.timeline,
        Some(json!([{"at": "Making", "event": "The maker ran."}]))
    );
    assert_eq!(detail.assessment_ids, Some(json!(["a-1", "a-2"])));
    assert_eq!(
        detail.grades,
        Some(json!([{"dimension": "correctness", "grade": 4}]))
    );
    assert_eq!(detail.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(detail.rubric_version, Some(1));
    assert_eq!(
        detail.recommendations,
        (1..=3).map(recommendation).collect::<Vec<_>>()
    );
    let tokens = store.code_grades("1").unwrap();
    assert_eq!(
        tokens
            .iter()
            .map(|g| (g.trigger.as_str(), g.dimension.as_str()))
            .collect::<Vec<_>>(),
        [("postmortem:1", "tokens")]
    );
    let mut client = raw(settings);
    let retained: Vec<serde_json::Value> = client
        .query("SELECT item FROM tl3_memory_outbox", &[])
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(
        retained,
        [json!({"content": "lesson", "tags": ["postmortem"]})]
    );
    let finished: bool = client
        .query_one(
            "SELECT finished_at IS NOT NULL AND started_at <= finished_at FROM tl3_postmortems WHERE id = $1",
            &[&first.id],
        )
        .unwrap()
        .get(0);
    assert!(finished);
    // The table refuses a fourth recommendation.
    let second = store.owed_postmortem().unwrap().unwrap();
    update(&mut store, "claim-2", claim(second.id, "postmortem:2:a")).unwrap();
    assert!(
        update(
            &mut store,
            "four",
            PostmortemUpdate::Done {
                id: second.id,
                run_key: "postmortem:2:a".into(),
                result: Box::new(judged(4)),
            },
        )
        .is_err()
    );
    assert_eq!(
        store.postmortem(second.id).unwrap().unwrap().record.status,
        "running"
    );
    // A failure keeps only its fixed class; a release makes it owed again.
    update(
        &mut store,
        "failed",
        PostmortemUpdate::Failed {
            id: second.id,
            run_key: "postmortem:2:a".into(),
            judge_failure: None,
            failure: "invalid_reply".into(),
        },
    )
    .unwrap();
    let failed = store.postmortem(second.id).unwrap().unwrap();
    assert_eq!(
        (failed.record.status.as_str(), failed.failure.as_deref()),
        ("failed", Some("invalid_reply"))
    );
    let snapshot = store.load().unwrap();
    let mut change = Commit::new("third", json!("third"), &snapshot);
    change.postmortems = vec![owed("rob:7")];
    store.commit(&change).unwrap();
    let third = store.owed_postmortem().unwrap().unwrap();
    update(&mut store, "claim-3", claim(third.id, "postmortem:3:a")).unwrap();
    update(
        &mut store,
        "release-stale",
        PostmortemUpdate::Release {
            id: third.id,
            run_key: "postmortem:3:z".into(),
        },
    )
    .unwrap();
    assert_eq!(
        store.postmortem(third.id).unwrap().unwrap().record.status,
        "running"
    );
    update(
        &mut store,
        "release",
        PostmortemUpdate::Release {
            id: third.id,
            run_key: "postmortem:3:a".into(),
        },
    )
    .unwrap();
    let released = store.postmortem(third.id).unwrap().unwrap();
    assert_eq!(
        (
            released.record.status.as_str(),
            released.record.run_key.as_deref()
        ),
        ("owed", None)
    );
    assert_eq!(store.owed_postmortem().unwrap().unwrap().id, third.id);
}

/// #486: a run that failed for a reason worth retrying is owed again, at most
/// twice, behind postmortems owed since. Claims count toward the UTC day they
/// were made in, and failed runs since the last success make a streak.
fn postmortem_retries_and_pace(settings: &tl3_config::Store) {
    let mut store = Store::open(settings).unwrap();
    let owed = |trigger: &str| OwedPostmortem {
        node: "1".into(),
        trigger: trigger.into(),
        reason: "needed_fix".into(),
        sample_share: None,
        repo: "owner/sandbox".into(),
        issue: 12,
        title: "Fix greeting".into(),
    };
    let mut change = seed(&mut store);
    change.postmortems = vec![owed("close")];
    store.commit(&change).unwrap();
    let start = store.load().unwrap().revision.get();
    let update = |store: &mut Store, key: &str, update: PostmortemUpdate| {
        let snapshot = store.load().unwrap();
        let mut change = Commit::new(key, json!(key), &snapshot);
        change.postmortem_updates = vec![update];
        store.commit(&change).unwrap();
    };
    let run = |store: &mut Store, id: i64, key: &str, failure: Option<&str>| {
        update(
            store,
            &format!("claim:{key}"),
            PostmortemUpdate::Claim {
                id,
                run_key: key.into(),
                rubric_version: 1,
            },
        );
        update(
            store,
            &format!("end:{key}"),
            match failure {
                Some(failure) => PostmortemUpdate::Failed {
                    id,
                    run_key: key.into(),
                    judge_failure: None,
                    failure: failure.into(),
                },
                None => PostmortemUpdate::Done {
                    id,
                    run_key: key.into(),
                    result: Box::new(judged(1)),
                },
            },
        );
    };
    let first = store.owed_postmortem().unwrap().unwrap();
    assert_eq!(first.attempts, 0);
    run(
        &mut store,
        first.id,
        "postmortem:1:a",
        Some("worker_failed"),
    );
    let record = store.postmortem(first.id).unwrap().unwrap();
    assert_eq!(
        (
            record.record.status.as_str(),
            record.record.attempts,
            record.record.run_key.as_deref(),
            record.failure.as_deref()
        ),
        ("owed", 1, None, Some("worker_failed")),
        "a worker failure is owed again"
    );
    // The oldest owed postmortem stays first, including its retries (#642).
    let snapshot = store.load().unwrap();
    let mut change = Commit::new("newer", json!("newer"), &snapshot);
    change.postmortems = vec![owed("question:3")];
    store.commit(&change).unwrap();
    assert_eq!(store.owed_postmortem().unwrap().unwrap().id, first.id);
    let next = store
        .postmortems("1")
        .unwrap()
        .into_iter()
        .find(|record| record.trigger == "question:3")
        .unwrap();
    run(&mut store, next.id, "postmortem:2:a", Some("invalid_reply"));
    assert_eq!(
        store.postmortem(next.id).unwrap().unwrap().record.status,
        "failed",
        "an invalid reply is not retried"
    );
    let retry = store.owed_postmortem().unwrap().unwrap();
    assert_eq!((retry.id, retry.attempts), (first.id, 1));
    run(&mut store, first.id, "postmortem:1:b", Some("deadline"));
    assert_eq!(
        store.postmortem(first.id).unwrap().unwrap().record.attempts,
        2,
        "a deadline is retried"
    );
    let middle = store.load().unwrap().revision.get();
    run(
        &mut store,
        first.id,
        "postmortem:1:c",
        Some("worker_failed"),
    );
    let record = store.postmortem(first.id).unwrap().unwrap();
    assert_eq!(
        (record.record.status.as_str(), record.record.attempts),
        ("failed", 2),
        "the third failure is final"
    );
    assert!(store.owed_postmortem().unwrap().is_none());
    assert_eq!(store.postmortem_claims_today().unwrap(), 4);
    assert_eq!(
        store.postmortem_failure_streak(start).unwrap(),
        ["worker_failed", "deadline", "invalid_reply"],
        "the failed runs since the last success, newest first, up to 3"
    );
    assert_eq!(
        store.postmortem_failure_streak(middle).unwrap(),
        ["worker_failed"]
    );
    raw(settings)
        .execute(
            "UPDATE tl3_postmortem_claims SET claimed_at = claimed_at - interval '1 day' WHERE run_key = 'postmortem:1:a'",
            &[],
        )
        .unwrap();
    assert_eq!(
        store.postmortem_claims_today().unwrap(),
        3,
        "yesterday's claim no longer counts"
    );
    let snapshot = store.load().unwrap();
    let mut change = Commit::new("third", json!("third"), &snapshot);
    change.postmortems = vec![owed("rob:7")];
    store.commit(&change).unwrap();
    let third = store.owed_postmortem().unwrap().unwrap();
    run(&mut store, third.id, "postmortem:3:a", None);
    assert!(
        store.postmortem_failure_streak(start).unwrap().is_empty(),
        "a success ends the streak"
    );
    // A run that succeeds after a retried failure shows no stale failure.
    let snapshot = store.load().unwrap();
    let mut change = Commit::new("fourth", json!("fourth"), &snapshot);
    change.postmortems = vec![owed("rob:8")];
    store.commit(&change).unwrap();
    let fourth = store.owed_postmortem().unwrap().unwrap();
    run(
        &mut store,
        fourth.id,
        "postmortem:4:a",
        Some("worker_failed"),
    );
    run(&mut store, fourth.id, "postmortem:4:b", None);
    let done = store.postmortem(fourth.id).unwrap().unwrap();
    assert_eq!(
        (done.record.status.as_str(), done.failure.as_deref()),
        ("done", None)
    );
    let failed_at: bool = raw(settings)
        .query_one(
            "SELECT failed_at IS NULL FROM tl3_postmortems WHERE id = $1",
            &[&fourth.id],
        )
        .unwrap()
        .get(0);
    assert!(failed_at, "done clears failed_at");
}
