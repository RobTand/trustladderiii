//! #783 / Q27: cached checks conform to the real full kernel on every observation.
use tl3_controller::{incremental::Incremental, verification::check_world};
use tl3_kernel::{Command, Event, Fact, NodeId, Priority, RepoId, World};
const _REGRESSIONS: &str = include_str!("incremental/proptest-regressions.txt");

fn world() -> World {
    let mut w = World::new(&[2], 2);
    for _ in 0..2 {
        w = tl3_kernel::step(
            w,
            Event::NewRequest {
                repo: RepoId(0),
                priority: Priority::Unranked,
            },
        )
        .0;
    }
    w
}
/// Seeded mutant regression: changing pool usage must invalidate its summary.
#[test]
fn pool_summary_regression_0783() {
    let mut w = world();
    let mut checker = Incremental::default();
    assert_eq!(checker.check(&w), check_world(&w));
    w.nodes[0].pool = false;
    w.pool_capacity = 1;
    assert_eq!(
        checker.check(&w),
        check_world(&w),
        "a stale pool summary must be detected"
    );
}
/// #783: deterministic generated event schedules plus arbitrary invalid snapshots.
#[test]
fn generated_sequences_match_full_checker() {
    use proptest::{
        prelude::*,
        test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
    };
    let mut runner = TestRunner::new(Config {
        cases: 128,
        rng_seed: RngSeed::Fixed(0x0783_2026_1104),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/incremental/proptest-regressions.txt"
        )))),
        ..Config::default()
    });
    runner
        .run(
            &proptest::collection::vec((any::<u8>(), any::<u8>()), 1..64),
            |(sequence)| {
                let mut w = world();
                let mut checker = Incremental::default();
                for (action, mutation) in sequence {
                    let index = action as usize % w.nodes.len();
                    let node = NodeId(index as u64);
                    let event = match action % 5 {
                        0 if w.nodes.len() < 24 => Event::NewRequest {
                            repo: RepoId(0),
                            priority: Priority::Unranked,
                        },
                        1 => Event::Fact {
                            node,
                            fact: Fact::Priority(Priority::P0),
                        },
                        2 => Event::Command {
                            node,
                            id: w.nodes[index].last_cmd + 1,
                            command: Command::Stop,
                        },
                        _ => w.witness().unwrap_or(Event::Fact {
                            node,
                            fact: Fact::Priority(Priority::P1),
                        }),
                    };
                    w = tl3_kernel::step(w, event).0;
                    prop_assert_eq!(checker.check(&w), check_world(&w));
                    let mut invalid = w.clone();
                    match mutation % 8 {
                        0 => invalid.pool_capacity = 0,
                        1 => invalid.gates[0].w = 0,
                        2 => invalid.nodes[index].repo = RepoId(999),
                        3 => invalid.nodes[index].pos = u8::MAX,
                        4 => invalid.nodes[index].pool = !invalid.nodes[index].pool,
                        5 => invalid.next_ticket = 0,
                        6 => invalid.nodes[index].holds = !invalid.nodes[index].holds,
                        _ => invalid.nodes[index].credit.withheld = true,
                    }
                    prop_assert_eq!(checker.check(&invalid), check_world(&invalid));
                    // Cache rollback: a failed candidate must not poison the next check.
                    prop_assert_eq!(checker.check(&w), check_world(&w));
                }
                Ok(())
            },
        )
        .unwrap();
}

/// #783: every periodic baseline plus its compact deltas reconstructs full snapshots.
#[test]
fn replay_from_every_baseline_matches_full_snapshots() {
    use proptest::{
        prelude::*,
        test_runner::{Config, RngSeed, TestRunner},
    };
    use tl3_controller::{
        compact::{CompactStep, Delta, Record},
        incremental::BASELINE_INTERVAL,
    };
    let mut runner = TestRunner::new(Config {
        cases: 24,
        rng_seed: RngSeed::Fixed(0x0783_2026_1204),
        failure_persistence: None,
        ..Config::default()
    });
    runner
        .run(
            &proptest::collection::vec(any::<u8>(), 130..260),
            |sequence| {
                let mut w = world();
                let mut records = Vec::new();
                let mut snapshots = Vec::new();
                let mut baseline_revision = 1;
                for (i, action) in sequence.into_iter().enumerate() {
                    let before = w.clone();
                    let node = NodeId(action as u64 % w.nodes.len() as u64);
                    let event = if action % 3 == 0 {
                        w.witness().unwrap_or(Event::Fact {
                            node,
                            fact: Fact::Priority(Priority::P0),
                        })
                    } else {
                        Event::Fact {
                            node,
                            fact: Fact::Priority(Priority::P1),
                        }
                    };
                    w = tl3_kernel::step(w, event).0;
                    let baseline = (i as u64).is_multiple_of(BASELINE_INTERVAL);
                    if baseline {
                        baseline_revision = i as i64 + 1;
                    }
                    let record = Record {
                        format: 1,
                        baseline: baseline.then(|| before.clone()),
                        baseline_revision,
                        previous_kernel_revision: i.checked_sub(1).map(|n| n as i64 + 1),
                        event_count: 1,
                        steps: vec![CompactStep {
                            preparation: Delta::between(&before, &before),
                            event,
                            after: Delta::between(&before, &w),
                        }],
                        settle: None,
                        final_delta: Delta::between(&w, &w),
                    };
                    records.push(record);
                    snapshots.push(w.clone());
                }
                for start in (0..records.len()).step_by(BASELINE_INTERVAL as usize) {
                    let mut prior = None;
                    for i in start..records.len() {
                        let actual = tl3_controller::compact::check(
                            &records[i],
                            prior.as_ref(),
                            i as i64 + 1,
                        )
                        .unwrap();
                        prop_assert_eq!(&actual, &snapshots[i]);
                        prior = Some(actual);
                    }
                }
                let mut missing = records[1].clone();
                missing.steps.clear();
                prop_assert!(
                    tl3_controller::compact::check(&missing, Some(&snapshots[0]), 2)
                        .unwrap_err()
                        .contains("unverifiable from revision 2")
                );
                prop_assert!(
                    tl3_controller::compact::check(&records[1], None, 2)
                        .unwrap_err()
                        .contains("baseline required")
                );
                Ok(())
            },
        )
        .unwrap();
}

/// #783: node-local corruption also round-trips, without normalizing invalid worlds.
#[test]
fn invalid_world_delta_roundtrip_and_constant_size() {
    use tl3_controller::compact::Delta;
    let before = world();
    let mut after = before.clone();
    after.nodes[0].pos = u8::MAX;
    let delta = Delta::between(&before, &after);
    let mut restored = before.clone();
    delta.apply(&mut restored).unwrap();
    assert_eq!(restored, after);
    assert_eq!(check_world(&restored), check_world(&after));
    let mut sizes = Vec::new();
    for count in [256, 1024, 4096] {
        let mut before = before.clone();
        let mut closed = tl3_kernel::Node::new(RepoId(0));
        closed.phase = tl3_kernel::Phase::Closed(tl3_kernel::Outcome::Stopped);
        before.nodes.resize(count, closed);
        let mut after = before.clone();
        after.nodes[0].priority = Priority::P0;
        let delta = Delta::between(&before, &after);
        assert_eq!(delta.nodes.puts.len(), 1);
        sizes.push(serde_json::to_vec(&delta).unwrap().len());
    }
    assert!(
        sizes.iter().max().unwrap() - sizes.iter().min().unwrap() < 16,
        "ordinary delta grew with retained nodes: {sizes:?}"
    );
}
