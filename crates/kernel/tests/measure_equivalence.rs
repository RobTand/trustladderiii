//! #806: preserve the diagnostic measure's pre-rewrite iterator behavior.
use tl3_kernel::*;

#[path = "measure_equivalence/reference.rs"]
mod reference;
use reference::compare;

/// #806: generated worlds exercise all statuses with both open and closed
/// owners, all phases, split flags and resumes, and mixed diagnostic state.
#[test]
fn index_measure_matches_the_original_iterator_over_generated_worlds() {
    compare(&World::new(&[1], 1));
    let phases = [
        Phase::Ingesting,
        Phase::Auditing,
        Phase::Unplanned,
        Phase::Planning { must_split: false },
        Phase::Planning { must_split: true },
        Phase::Ready(Resume::Making),
        Phase::Ready(Resume::Checking),
        Phase::Ready(Resume::Reviewing),
        Phase::Ready(Resume::Publishing),
        Phase::Ready(Resume::Rebasing),
        Phase::Making,
        Phase::Checking,
        Phase::Reviewing,
        Phase::Publishing,
        Phase::Queued,
        Phase::Rebasing,
        Phase::AwaitingRob,
        Phase::Stopping(Outcome::Stopped),
        Phase::Closed(Outcome::Stopped),
        Phase::Closed(Outcome::Merged),
    ];
    let statuses = [
        DependencyStatus::Pending,
        DependencyStatus::Delivered,
        DependencyStatus::Abandoned,
    ];
    let mut seen = [[false; 3]; 2];
    let mut comparisons = 0;
    for seed in 1..=128usize {
        let mut world = World::new(&[3], 2);
        for _ in 0..1 + seed % 8 {
            world = step(
                world,
                Event::NewRequest {
                    repo: RepoId(0),
                    priority: Priority::Unranked,
                },
            )
            .0;
        }
        for i in 0..world.nodes.len() {
            for (status_index, &status) in statuses.iter().enumerate() {
                world = step(
                    world,
                    Event::Fact {
                        node: NodeId(i as u64),
                        fact: Fact::Dependency {
                            target: DependencyTarget {
                                kind: PrerequisiteKind::Issue,
                                number: (3 * i + status_index + 1) as u64,
                                request: None,
                            },
                            status,
                        },
                    },
                )
                .0;
            }
        }
        assert_eq!(world.dependencies().len(), 3 * world.nodes.len());
        for shift in 0..phases.len() {
            for (i, node) in world.nodes.iter_mut().enumerate() {
                node.phase = phases[(seed + shift + i) % phases.len()];
                node.pos = ((seed + shift + i) % 8) as u8;
                node.paused = (seed + shift + i) % 2 == 0;
                node.waiting_incident = (seed + shift + i) % 3 == 0;
            }
            for dependency in world.dependencies() {
                let owner = &world.nodes[dependency.node.0 as usize];
                let status = match dependency.status {
                    DependencyStatus::Pending => 0,
                    DependencyStatus::Delivered => 1,
                    DependencyStatus::Abandoned => 2,
                };
                seen[usize::from(owner.phase.is_closed())][status] = true;
            }
            compare(&world);
            comparisons += 1;
        }
    }
    assert_eq!(seen, [[true; 3]; 2]);
    assert_eq!(comparisons, 2560);
    println!("measure equivalence: {comparisons} generated worlds, all status/owner combinations");
}
