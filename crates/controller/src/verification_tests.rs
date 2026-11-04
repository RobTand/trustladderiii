//! #783 / Q27: detection must leave the main transaction and effects unchanged.
use crate::{git_tests::support::Database, *};
use tl3_kernel::{Event, Fact, NodeId, Priority, RepoId};
use tl3_store::{Commit, RowChange, Store};

struct FixedClock;
impl clock::Clock for FixedClock {
    fn now(&self) -> std::time::SystemTime {
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000)
    }
}
fn bootstrap() -> Bootstrap {
    Bootstrap {
        operations: Some(tl3_config::OperationsIssue {
            owner: "owner".into(),
            name: "ops".into(),
            installation_id: 1,
            number: 99,
        }),
        repos: vec![Repository {
            owner: "owner".into(),
            name: "sandbox".into(),
            installation: 1,
        }],
        rob: 42,
        slots: 1,
        pool: 2,
        fail_fast: false,
    }
}
fn intake(c: &mut Controller, number: u64) {
    c.intake(LabelledIssue {
        delivery: format!("fixture:{number}"),
        owner: "owner".into(),
        repo: "sandbox".into(),
        installation: 1,
        number,
        title: "Fixture".into(),
        actor: 42,
        label: "tl3".into(),
        labels: vec![],
    })
    .unwrap();
}
fn without_instrumentation(mut c: Commit) -> String {
    c.rows
        .retain(|r| !matches!(r,RowChange::Put {table,..} if table == "kernel_alarms"));
    if let Some(result) = c.result.as_object_mut() {
        for key in ["kernel_replay", "kernel_steps", "kernel_alarms"] {
            result.remove(key);
        }
    }
    format!("{c:?}")
}

/// Q27 and state-table row 139: both valid and invalid outputs commit unchanged.
#[test]
fn detection_preserves_main_transaction_and_effects() {
    let mut db = Database::start();
    let settings = db.create("invariant_equivalence");
    let root = tempfile::tempdir().unwrap();
    let mut c = Controller::open_with_clock(
        Store::open(&settings).unwrap(),
        adapters::simulation(root.path()).unwrap(),
        bootstrap(),
        FixedClock,
    )
    .unwrap();
    intake(&mut c, 42);
    intake(&mut c, 43);
    let s = c.snapshot().unwrap();
    let world = c.world().unwrap();
    for invalid in [false, true] {
        let mut input = world.clone();
        if invalid {
            input.pool_capacity = 0;
        }
        let event = Event::Fact {
            node: NodeId(0),
            fact: Fact::Priority(Priority::P0),
        };
        let mut actual = Commit::new("fixture:equivalence", json!({}), &s);
        let mut expected = actual.clone();
        // Include pre-existing inbox writes, consumed completion transitions and effects.
        actual
            .rows
            .push(put("inbox", "retained-fact".into(), json!({"fact":true})));
        expected.rows = actual.rows.clone();
        c.apply_reference(&s, input.clone(), event, &mut expected)
            .unwrap();
        c.apply(&s, input, event, &mut actual).unwrap();
        assert_eq!(
            without_instrumentation(actual.clone()),
            without_instrumentation(expected),
            "invalid={invalid}: detector changed main transaction"
        );
        if invalid {
            assert_eq!(
                actual
                    .rows
                    .iter()
                    .filter(|r| matches!(r,RowChange::Put {table,..} if table=="kernel_alarms"))
                    .count(),
                1,
                "exactly one visible alarm"
            );
        }
    }
    // New-request output includes a real StartRun effect and its allowance.
    let event = Event::NewRequest {
        repo: RepoId(0),
        priority: Priority::Unranked,
    };
    let mut actual = Commit::new("fixture:effect", json!({}), &s);
    let mut expected = actual.clone();
    c.apply_reference(&s, world.clone(), event, &mut expected)
        .unwrap();
    c.apply(&s, world, event, &mut actual).unwrap();
    assert!(!actual.effects.is_empty());
    assert_eq!(
        without_instrumentation(actual),
        without_instrumentation(expected)
    );
}

/// #783: one alarm per (request, invariant), durable across Store/controller reopen.
#[test]
fn alarm_survives_restart_and_does_not_duplicate_or_reject() {
    let mut db = Database::start();
    let settings = db.create("invariant_alarm_restart");
    let root = tempfile::tempdir().unwrap();
    let open = || {
        Controller::open_with_clock(
            Store::open(&settings).unwrap(),
            adapters::simulation(root.path()).unwrap(),
            bootstrap(),
            FixedClock,
        )
        .unwrap()
    };
    let mut c = open();
    intake(&mut c, 42);
    let baseline = c.world().unwrap();
    let s = c.snapshot().unwrap();
    let mut bad = baseline.clone();
    bad.pool_capacity = 0;
    let event = Event::Fact {
        node: NodeId(0),
        fact: Fact::Priority(Priority::P0),
    };
    let expected = tl3_kernel::step(bad.clone(), event).0;
    let mut commit = Commit::new("fixture:invalid", json!({}), &s);
    c.apply(&s, bad, event, &mut commit).unwrap();
    c.store.commit(&commit).unwrap();
    let s = c.snapshot().unwrap();
    for ((table, id), value) in snapshot::encode(&expected).unwrap() {
        assert_eq!(
            s.rows
                .iter()
                .find(|r| r.table == table && r.id == id)
                .map(|r| &r.value),
            Some(&value),
            "invalid world must be committed as on main"
        );
    }
    assert!(
        !s.rows
            .iter()
            .any(|r| matches!(r.table.as_str(), "incidents" | "kernel_holds"))
    );
    let alarm = s
        .rows
        .iter()
        .find(|r| r.table == "kernel_alarms")
        .expect("durable alarm");
    assert!(alarm.value.to_string().contains("fixture:invalid"));
    // Existing main startup rejects invalid snapshots. Repair only the test fixture
    // before restarting the controller; the alarm itself must remain durable.
    let mut repair = Commit::new("fixture:repair", json!({}), &s);
    repair.rows = snapshot::changes(
        &snapshot::encode(&expected).unwrap(),
        snapshot::encode(&baseline).unwrap(),
    );
    c.store.commit(&repair).unwrap();
    drop(c);
    let mut c = open();
    let s = c.snapshot().unwrap();
    let mut bad = baseline;
    bad.pool_capacity = 0;
    let mut repeated = Commit::new("fixture:invalid-again", json!({}), &s);
    c.apply(&s, bad, event, &mut repeated).unwrap();
    c.store.commit(&repeated).unwrap();
    assert_eq!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.table == "kernel_alarms")
            .count(),
        1
    );
}
