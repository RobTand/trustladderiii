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
