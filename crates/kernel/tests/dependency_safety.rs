//! K4a executable covers for the arbitrary-world dependency theorems:
//! registration, removal, no-cycle, waiting-holds-nothing and
//! delivered-releases, on both graph traversal branches. Every step goes
//! through the public `step` and the full checker (`valid`).
use tl3_kernel::*;

fn send(world: &mut World, event: Event) -> Vec<Out> {
    let StepResult {
        world: next,
        outputs: out,
        ..
    } = step(world.clone(), event);
    assert!(next.valid(), "invalid after {event:?}: {next:#?}");
    *world = next;
    out
}

fn confirm(world: &mut World, node: NodeId) {
    for _ in 0..32 {
        let Some(effect) = world.nodes[node.0 as usize].effect else {
            return;
        };
        send(
            world,
            Event::Ack {
                node,
                seq: effect.seq,
                ack: if effect.kind == EffectKind::Enqueue {
                    Ack::DeliveryAccepted
                } else {
                    Ack::Confirmed
                },
            },
        );
    }
    panic!("effects did not settle");
}

fn reply(world: &mut World, node: NodeId, reply: Reply) {
    confirm(world, node);
    let generation = world.nodes[node.0 as usize].generation;
    send(
        world,
        Event::Reply {
            node,
            generation,
            reply,
        },
    );
    confirm(world, node);
}

fn roots(count: usize) -> World {
    let mut world = World::new(&[3], 3);
    for i in 0..count {
        send(
            &mut world,
            Event::NewRequest {
                repo: RepoId(0),
                priority: Priority::Unranked,
            },
        );
        confirm(&mut world, NodeId(i as u64));
    }
    world
}

fn target(number: u64, request: Option<NodeId>) -> DependencyTarget {
    DependencyTarget {
        kind: PrerequisiteKind::Issue,
        number,
        request,
    }
}

fn declare(
    world: &mut World,
    node: NodeId,
    target: DependencyTarget,
    status: DependencyStatus,
) -> Vec<Out> {
    send(
        world,
        Event::Fact {
            node,
            fact: Fact::Dependency { target, status },
        },
    )
}

fn remove(world: &mut World, node: NodeId, number: u64) -> Vec<Out> {
    send(
        world,
        Event::Fact {
            node,
            fact: Fact::DependencyRemove {
                kind: PrerequisiteKind::Issue,
                number,
            },
        },
    )
}

fn audit(world: &mut World, node: NodeId) {
    reply(world, node, Reply::IngestDone { size: 1 });
    reply(world, node, Reply::AuditAgrees);
}

fn row(world: &World, node: u64, number: u64) -> Dependency {
    *world
        .dependencies()
        .iter()
        .find(|d| d.node == NodeId(node) && d.target.number == number)
        .expect("declaration present")
}

fn chain(count: u64) -> World {
    let mut world = roots(count as usize);
    for node in 1..count {
        declare(
            &mut world,
            NodeId(node),
            target(node, Some(NodeId(node - 1))),
            DependencyStatus::Pending,
        );
        assert!(!row(&world, node, node).rejected);
    }
    assert!(world.dependency_graph_acyclic());
    world
}

/// no-cycle, both branches: a cycle-closing request is rejected and an
/// acyclic one accepted on 64 roots (bitset) and on 65 roots (heap).
#[test]
fn cycle_decision_is_exact_on_both_traversal_branches() {
    for count in [64u64, 65] {
        let mut world = chain(count);
        let last = count - 1;
        declare(
            &mut world,
            NodeId(0),
            target(1000, Some(NodeId(last))),
            DependencyStatus::Pending,
        );
        assert!(row(&world, 0, 1000).rejected, "cycle accepted at {count}");
        declare(
            &mut world,
            NodeId(last),
            target(1001, Some(NodeId(0))),
            DependencyStatus::Pending,
        );
        assert!(!row(&world, last, 1001).rejected, "acyclic rejected at {count}");
        assert!(world.dependency_graph_acyclic());
    }
}

/// no-cycle: delivered and rejected declarations are not graph edges.
#[test]
fn delivered_and_rejected_rows_are_not_edges() {
    let mut world = roots(2);
    declare(
        &mut world,
        NodeId(0),
        target(2752, Some(NodeId(1))),
        DependencyStatus::Pending,
    );
    declare(
        &mut world,
        NodeId(1),
        target(2751, Some(NodeId(0))),
        DependencyStatus::Pending,
    );
    assert!(row(&world, 1, 2751).rejected);
    // The rejected 1->0 row adds no edge, so 0->1 stays accepted after delivery.
    declare(
        &mut world,
        NodeId(0),
        target(2752, Some(NodeId(1))),
        DependencyStatus::Delivered,
    );
    declare(
        &mut world,
        NodeId(1),
        target(2753, Some(NodeId(0))),
        DependencyStatus::Pending,
    );
    assert!(!row(&world, 1, 2753).rejected);
    assert!(world.dependency_graph_acyclic());
}

/// registration: an unstarted root gains exactly one row; a started one none.
#[test]
fn registration_appends_only_before_planning() {
    let mut world = roots(2);
    declare(
        &mut world,
        NodeId(0),
        target(2762, None),
        DependencyStatus::Pending,
    );
    assert_eq!(world.dependencies().len(), 1);
    audit(&mut world, NodeId(1));
    assert_eq!(world.nodes[1].phase, Phase::Planning { must_split: false });
    assert!(world.nodes[1].planning_started);
    for status in [
        DependencyStatus::Pending,
        DependencyStatus::Delivered,
        DependencyStatus::Abandoned,
    ] {
        declare(&mut world, NodeId(1), target(2763, None), status);
        assert_eq!(world.dependencies().len(), 1, "started root gained {status:?}");
    }
}

/// removal: re-ingest drops only the matching key; other rows keep identity,
/// terminal outcome and order; other phases keep every row.
#[test]
fn removal_is_a_stable_filter_only_while_ingesting() {
    let mut world = roots(2);
    declare(&mut world, NodeId(0), target(1, None), DependencyStatus::Pending);
    declare(&mut world, NodeId(1), target(2, None), DependencyStatus::Pending);
    declare(&mut world, NodeId(0), target(3, None), DependencyStatus::Pending);
    declare(&mut world, NodeId(0), target(3, None), DependencyStatus::Abandoned);
    declare(&mut world, NodeId(0), target(4, None), DependencyStatus::Delivered);
    let before: Vec<Dependency> = world.dependencies().to_vec();
    remove(&mut world, NodeId(0), 1);
    let kept: Vec<Dependency> = before
        .iter()
        .copied()
        .filter(|d| !(d.node == NodeId(0) && d.target.number == 1))
        .collect();
    assert_eq!(world.dependencies(), kept.as_slice());
    assert_eq!(row(&world, 0, 3).status, DependencyStatus::Abandoned);
    audit(&mut world, NodeId(1));
    let unchanged: Vec<Dependency> = world.dependencies().to_vec();
    remove(&mut world, NodeId(1), 2);
    assert_eq!(world.dependencies(), unchanged.as_slice());
}

/// waiting-holds-nothing on a non-fixture world: the second of three roots
/// waits on an in-world request and holds nothing, and its balance is zero.
#[test]
fn waiting_owner_in_a_larger_world_holds_nothing() {
    let mut world = roots(3);
    declare(
        &mut world,
        NodeId(1),
        target(2750, Some(NodeId(0))),
        DependencyStatus::Pending,
    );
    for node in 0..3 {
        audit(&mut world, NodeId(node));
    }
    let n = &world.nodes[1];
    assert_eq!(n.phase, Phase::Unplanned);
    assert!(!n.holds && !n.pool && n.ticket.is_none() && n.run.is_none());
    assert_eq!(n.credit.balance(), 0);
    assert_eq!(world.nodes[2].phase, Phase::Planning { must_split: false });
}

/// delivered-releases on a non-fixture world: delivery admits the waiter
/// with its ledger intact while other roots keep their state.
#[test]
fn delivery_releases_one_waiter_among_others() {
    let mut world = roots(3);
    let prerequisite = target(2750, Some(NodeId(0)));
    declare(&mut world, NodeId(2), prerequisite, DependencyStatus::Pending);
    for node in 0..3 {
        audit(&mut world, NodeId(node));
    }
    assert_eq!(world.nodes[2].phase, Phase::Unplanned);
    let other = world.nodes[1];
    declare(&mut world, NodeId(2), prerequisite, DependencyStatus::Delivered);
    assert_eq!(world.nodes[2].phase, Phase::Planning { must_split: false });
    assert!(world.nodes[2].planning_started);
    assert_eq!(world.nodes[2].credit.balance(), 4);
    assert_eq!(world.nodes[1].phase, other.phase);
}
