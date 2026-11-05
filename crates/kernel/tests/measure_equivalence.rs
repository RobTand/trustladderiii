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

fn roots() -> World {
    let mut w = World::new(&[3], 2);
    for _ in 0..2 {
        w = step(
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

fn dependency(w: World, owner: u64, request: Option<NodeId>, status: DependencyStatus) -> World {
    step(
        w,
        Event::Fact {
            node: NodeId(owner),
            fact: Fact::Dependency {
                target: DependencyTarget {
                    kind: PrerequisiteKind::Issue,
                    number: owner + 1,
                    request,
                },
                status,
            },
        },
    )
    .0
}

/// #806 review r1: retain rejected cycle declarations in diagnostic comparisons.
#[test]
fn rejected_cycles_match_the_iterator() {
    let mut w = dependency(roots(), 0, Some(NodeId(1)), DependencyStatus::Pending);
    w = dependency(w, 1, Some(NodeId(0)), DependencyStatus::Pending);
    assert_eq!(w.dependencies().len(), 2);
    assert!(w.dependencies().iter().all(|d| d.target.request.is_some()));
    assert!(w.dependencies().iter().any(|d| d.rejected));
    for closed in [false, true] {
        w.nodes[1].phase = if closed {
            Phase::Closed(Outcome::Stopped)
        } else {
            Phase::Unplanned
        };
        compare(&w);
        assert!(reference::compare_outcome(&w).is_some());
    }
}

/// #806 review r1: Delivered must short-circuit even with an invalid owner.
#[test]
fn truncated_owners_have_the_same_panic_or_value() {
    for status in [
        DependencyStatus::Pending,
        DependencyStatus::Abandoned,
        DependencyStatus::Delivered,
    ] {
        let mut w = dependency(roots(), 1, Some(NodeId(0)), status);
        assert_eq!(w.dependencies().len(), 1);
        w.nodes.truncate(1);
        assert!(w.dependencies()[0].node.0 as usize >= w.nodes.len());
        let outcome = reference::compare_outcome(&w);
        if status == DependencyStatus::Delivered {
            assert_eq!(outcome, Some(w.nodes[0].distance()));
        } else {
            assert_eq!(outcome, None);
        }
    }
}

/// #806 review r1: a real world near u64::MAX would need quadrillions of
/// nodes (distance <= 4096). Seed the two accumulation shapes instead, using
/// real world contributions. The original reference above remains verbatim.
#[test]
fn accumulator_boundary_has_the_same_panic_or_value() {
    let mut w = dependency(roots(), 0, Some(NodeId(1)), DependencyStatus::Pending);
    w = dependency(w, 1, Some(NodeId(0)), DependencyStatus::Pending);
    for n in &mut w.nodes {
        n.phase = Phase::Ingesting;
        n.paused = true;
        n.waiting_incident = true;
        n.credit.start = u64::MAX;
        n.credit.earned = u64::MAX;
        n.credit.added = u64::MAX;
    }
    assert!(reference::compare_outcome(&w).is_some());
    let node_sum: u64 = w.nodes.iter().map(Node::distance).sum();
    let edge_count = w.dependencies().len() as u64;
    assert!(node_sum > 0 && edge_count > 0);
    let mut successful = 0;
    let mut panicked = 0;
    // Exact fit, overflow only at the final edge addition, and overflow in
    // the node scan. Ordinary additions retain the build's overflow policy.
    for seed in [
        u64::MAX - node_sum - edge_count,
        u64::MAX - node_sum,
        u64::MAX,
    ] {
        let old = std::panic::catch_unwind(|| {
            let mut total = seed;
            for n in &w.nodes {
                total += n.distance();
            }
            total += w
                .dependencies()
                .iter()
                .filter(|d| {
                    d.status != DependencyStatus::Delivered
                        && !w.nodes[d.node.0 as usize].phase.is_closed()
                })
                .count() as u64;
            total
        })
        .ok();
        let new = std::panic::catch_unwind(|| {
            let mut total = seed;
            let mut i = 0;
            while i < w.nodes.len() {
                total += w.nodes[i].distance();
                i += 1;
            }
            let mut count = 0usize;
            let mut i = 0;
            while i < w.dependencies().len() {
                let d = &w.dependencies()[i];
                if d.status != DependencyStatus::Delivered
                    && !w.nodes[d.node.0 as usize].phase.is_closed()
                {
                    count += 1;
                }
                i += 1;
            }
            total += count as u64;
            total
        })
        .ok();
        assert_eq!(old, new, "initial total: {seed}");
        successful += usize::from(new.is_some());
        panicked += usize::from(new.is_none());
    }
    assert!(successful > 0);
    if cfg!(debug_assertions) {
        assert_eq!((successful, panicked), (1, 2));
    }
    println!("accumulator boundary: {successful} values, {panicked} panics");
}
