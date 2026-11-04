//! #783 / Q27: detection must leave the main transaction and effects unchanged.
use crate::{git_tests::support::Database, *};
use tl3_kernel::{Event, Fact, NodeId, Priority, RepoId, World};
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
        pool: 4,
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
        // Include pre-existing inbox writes, consumed completion transitions and effects.
        actual
            .rows
            .push(put("inbox", "retained-fact".into(), json!({"fact":true})));
        actual.transitions.push(tl3_store::EffectTransition {
            key: "fixture:completion".into(),
            from: tl3_store::EffectState::Uncertain,
            to: tl3_store::EffectState::Confirmed,
        });
        let mut expected = actual.clone();
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
    // Both valid and invalid new requests retain their StartRun effect and allowance.
    for invalid in [false, true] {
        let event = Event::NewRequest {
            repo: RepoId(0),
            priority: Priority::Unranked,
        };
        let mut input = world.clone();
        if invalid {
            input.gates[0].w = 0;
        }
        let mut actual = Commit::new("fixture:effect", json!({}), &s);
        let mut binding = s
            .rows
            .iter()
            .find(|r| r.table == "requests" && r.id == "0")
            .unwrap()
            .value
            .clone();
        binding["issue"] = json!(44);
        actual
            .rows
            .push(put("requests", world.nodes.len().to_string(), binding));
        let mut expected = actual.clone();
        c.apply_reference(&s, input.clone(), event, &mut expected)
            .unwrap();
        c.apply(&s, input, event, &mut actual).unwrap();
        assert!(!actual.effects.is_empty());
        assert_eq!(
            without_instrumentation(actual),
            without_instrumentation(expected)
        );
    }
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
    let status = crate::status::load(&mut c.store.reader().unwrap()).unwrap();
    assert!(crate::status::render_text(&status).contains("KERNEL INVARIANT ALARM"));
    assert!(crate::status::render_html(&status).contains("fixture:invalid"));
    assert!(c.post_kernel_alarm(&s).unwrap());
    let posted = c.snapshot().unwrap();
    assert!(!c.post_kernel_alarm(&posted).unwrap());
    let counts = adapters::delivery_counts(root.path()).unwrap();
    assert_eq!(counts.values().sum::<u64>(), 1, "one operations comment");
    // Simulate a comment landing before the local posted bit commits. Its
    // durable marker must reconcile, without another upstream post.
    let row = posted
        .rows
        .iter()
        .find(|r| r.table == "kernel_alarms")
        .unwrap();
    let mut unconfirmed = row.value.clone();
    unconfirmed["posted"] = json!(false);
    let mut crash = Commit::new("fixture:crash-before-posted", json!({}), &posted);
    crash
        .rows
        .push(put("kernel_alarms", row.id.clone(), unconfirmed));
    c.store.commit(&crash).unwrap();
    let s = c.snapshot().unwrap();
    assert!(c.post_kernel_alarm(&s).unwrap());
    assert_eq!(adapters::delivery_counts(root.path()).unwrap(), counts);
    let s = c.snapshot().unwrap();
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
    assert!(!c.post_kernel_alarm(&s).unwrap());
    assert_eq!(adapters::delivery_counts(root.path()).unwrap(), counts);
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

/// #783: paired measurements include the real apply/record path, snapshot-row
/// searches, predicate scans, all JSON construction, and final JSON wire encoding.
/// Fixture construction is outside the measurement. The store read already on
/// main is retained; there are no additional database reads or commits on this path.
#[test]
fn measure_complete_step_cost_and_allocations() {
    use std::hint::black_box;
    use tl3_kernel::{Node, Outcome, Phase};
    use tl3_store::{Epoch, Revision, Snapshot, WorldRow};
    let mut db = Database::start();
    let settings = db.create("invariant_cost");
    let root = tempfile::tempdir().unwrap();
    let mut c = Controller::open_with_clock(
        Store::open(&settings).unwrap(),
        adapters::simulation(root.path()).unwrap(),
        bootstrap(),
        FixedClock,
    )
    .unwrap();
    for density in [4, 16] {
        for count in [256, 1024, 4096] {
            let mut world = World::new(&[1], 2);
            let mut closed = Node::new(RepoId(0));
            closed.phase = Phase::Closed(Outcome::Stopped);
            world.nodes = vec![closed; count];
            assert!(world.valid());
            let mut rows: Vec<_> = snapshot::encode(&world)
                .unwrap()
                .into_iter()
                .map(|((table, id), value)| WorldRow {
                    table,
                    id,
                    value,
                    version: Revision::ZERO,
                    arrival: 0,
                })
                .collect();
            for node in 0..count {
                for table in 0..density {
                    rows.push(WorldRow {table:format!("fixture_metadata_{table}"),id:node.to_string(),value:json!({"state":"retained","node":node,"metadata":"representative payload"}),version:Revision::ZERO,arrival:0});
                }
            }
            let s = Snapshot {
                revision: Revision::ZERO,
                epoch: Epoch::ZERO,
                rows,
                pending_effects: vec![],
                credit: vec![],
                interned: vec![],
            };
            let event = Event::Fact {
                node: NodeId(0),
                fact: Fact::Priority(Priority::P0),
            };
            let mut totals = [0u128; 2];
            let iterations = 3;
            for turn in 0..iterations {
                // Alternate ordering to reduce cache/order bias.
                for mode in [turn % 2, 1 - turn % 2] {
                    let input = world.clone();
                    let mut commit = Commit::new("fixture:measured", json!({}), &s);
                    use std::{io::Write, time::Instant};
                    let probe = std::env::var_os("TL3_ALLOC_CHILD").is_some();
                    if probe {
                        eprintln!(
                            "allocation_case nodes={count} density={density} mode={mode} iteration={turn}"
                        );
                        std::io::stderr().write_all(b"TL3_ALLOC_START\n").unwrap();
                    }
                    let start = Instant::now();
                    {
                        if mode == 0 {
                            c.apply_reference(&s, input, event, &mut commit).unwrap();
                        } else {
                            assert!(!c.post_kernel_alarm(&s).unwrap());
                            c.apply(&s, input, event, &mut commit).unwrap();
                        }
                        // The store's JSONB parameter encoder serializes every row,
                        // effect payload and event result. Include those allocations.
                        for row in &commit.rows {
                            if let RowChange::Put { value, .. } = row {
                                black_box(serde_json::to_vec(value).unwrap());
                            }
                        }
                        for effect in &commit.effects {
                            black_box(serde_json::to_vec(&effect.payload).unwrap());
                        }
                        black_box(serde_json::to_vec(&commit.result).unwrap());
                        black_box(commit);
                    }
                    let ns = start.elapsed().as_nanos();
                    if probe {
                        std::io::stderr().write_all(b"TL3_ALLOC_STOP\n").unwrap();
                    }
                    totals[mode] += ns;
                }
            }
            let base = totals[0];
            let added = totals[1];
            println!(
                "complete_invariant_cost nodes={count} extra_rows_per_node={density} iterations={iterations} main_ns={} instrumented_ns={} added_ns={} added_allocations={} added_bytes={}",
                base.0 / iterations as u128,
                added.0 / iterations as u128,
                (added.0 as i128 - base.0 as i128) / iterations as i128,
                (added.1 as i128 - base.1 as i128) / iterations as i128,
                (added.2 as i128 - base.2 as i128) / iterations as i128
            );
        }
    }

    if std::env::var_os("TL3_COST_ALLOCATIONS").is_some()
        && std::env::var_os("TL3_ALLOC_CHILD").is_none()
    {
        let probe = std::env::current_dir()
            .unwrap()
            .join("crates/controller/tools/libtl3_allocation_counter.so");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "verification::tests::measure_complete_step_cost_and_allocations",
                "--nocapture",
            ])
            .env("LD_PRELOAD", probe)
            .env("TL3_ALLOC_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            stderr.matches("TL3_ALLOC_RESULT").count(),
            36,
            "external allocation probe did not observe every measured region: {stderr}"
        );
        println!("{stderr}");
    }
}
