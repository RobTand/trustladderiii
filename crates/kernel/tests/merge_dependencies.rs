use tl3_kernel::*;

fn send(world: &mut World, event: Event) -> Vec<Out> {
    let (next, out) = step(world.clone(), event);
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
                ack: Ack::Confirmed,
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

fn dependency(
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

fn audit(world: &mut World, node: NodeId) {
    reply(world, node, Reply::IngestDone { size: 1 });
    reply(world, node, Reply::AuditAgrees);
}

#[test]
fn open_prerequisite_waits_without_planning_or_resources() {
    let mut world = roots(1);
    dependency(
        &mut world,
        NodeId(0),
        target(2761, None),
        DependencyStatus::Pending,
    );
    audit(&mut world, NodeId(0));
    let n = &world.nodes[0];
    assert_eq!(n.phase, Phase::Unplanned);
    assert!(!n.holds && !n.pool && n.ticket.is_none() && n.run.is_none());
    assert!(n.footprint.is_empty());
    assert_eq!(n.credit.balance(), 0);
}

#[test]
fn delivered_prerequisite_releases_planning() {
    let mut world = roots(1);
    let prerequisite = target(2761, None);
    dependency(
        &mut world,
        NodeId(0),
        prerequisite,
        DependencyStatus::Pending,
    );
    audit(&mut world, NodeId(0));
    assert_eq!(world.nodes[0].phase, Phase::Unplanned);
    dependency(
        &mut world,
        NodeId(0),
        prerequisite,
        DependencyStatus::Delivered,
    );
    assert_eq!(world.nodes[0].phase, Phase::Planning { must_split: false });
    assert_eq!(world.nodes[0].credit.balance(), 4);
}

#[test]
fn not_planned_blocks_with_one_note_even_after_a_late_delivery() {
    let mut world = roots(1);
    let prerequisite = target(2761, None);
    dependency(
        &mut world,
        NodeId(0),
        prerequisite,
        DependencyStatus::Pending,
    );
    audit(&mut world, NodeId(0));
    let out = dependency(
        &mut world,
        NodeId(0),
        prerequisite,
        DependencyStatus::Abandoned,
    );
    assert_eq!(
        out.iter()
            .filter(|o| matches!(
                o,
                Out::Notice {
                    notice: Notice::Note(NoteKind::DependencyAbandoned { number: 2761 }),
                    ..
                }
            ))
            .count(),
        1
    );
    let repeated = dependency(
        &mut world,
        NodeId(0),
        prerequisite,
        DependencyStatus::Abandoned,
    );
    assert!(repeated.is_empty());
    dependency(
        &mut world,
        NodeId(0),
        prerequisite,
        DependencyStatus::Delivered,
    );
    assert_eq!(world.nodes[0].phase, Phase::Unplanned);
    assert_eq!(world.nodes[0].credit.balance(), 0);
}

#[test]
fn chain_releases_one_root_at_a_time() {
    let mut world = roots(3);
    let a = target(2751, Some(NodeId(0)));
    let b = target(2752, Some(NodeId(1)));
    dependency(&mut world, NodeId(1), a, DependencyStatus::Pending);
    dependency(&mut world, NodeId(2), b, DependencyStatus::Pending);
    for node in 0..3 {
        audit(&mut world, NodeId(node));
    }
    assert_eq!(world.nodes[0].phase, Phase::Planning { must_split: false });
    assert_eq!(world.nodes[1].phase, Phase::Unplanned);
    assert_eq!(world.nodes[2].phase, Phase::Unplanned);
    dependency(&mut world, NodeId(1), a, DependencyStatus::Delivered);
    assert_eq!(world.nodes[1].phase, Phase::Planning { must_split: false });
    assert_eq!(world.nodes[2].phase, Phase::Unplanned);
    dependency(&mut world, NodeId(2), b, DependencyStatus::Delivered);
    assert_eq!(world.nodes[2].phase, Phase::Planning { must_split: false });
}

#[test]
fn cycle_is_rejected_with_a_note_and_cannot_admit() {
    let mut world = roots(2);
    dependency(
        &mut world,
        NodeId(0),
        target(2752, Some(NodeId(1))),
        DependencyStatus::Pending,
    );
    let out = dependency(
        &mut world,
        NodeId(1),
        target(2751, Some(NodeId(0))),
        DependencyStatus::Pending,
    );
    assert!(out.iter().any(|o| matches!(
        o,
        Out::Notice {
            node: NodeId(1),
            notice: Notice::Note(NoteKind::DependencyCycle { number: 2751 })
        }
    )));
    audit(&mut world, NodeId(0));
    audit(&mut world, NodeId(1));
    assert!(world.nodes.iter().all(|n| n.phase == Phase::Unplanned));
}

#[test]
fn request_without_prerequisites_is_unchanged() {
    let mut world = roots(1);
    audit(&mut world, NodeId(0));
    assert_eq!(world.nodes[0].phase, Phase::Planning { must_split: false });
    assert_eq!(world.nodes[0].credit.balance(), 4);
}

#[test]
fn cycle_rejection_has_no_sixty_four_root_limit() {
    let mut world = roots(65);
    for node in 1..65 {
        dependency(
            &mut world,
            NodeId(node),
            target(node, Some(NodeId(node - 1))),
            DependencyStatus::Pending,
        );
    }
    let out = dependency(
        &mut world,
        NodeId(0),
        target(65, Some(NodeId(64))),
        DependencyStatus::Pending,
    );
    assert!(out.iter().any(|out| matches!(
        out,
        Out::Notice {
            notice: Notice::Note(NoteKind::DependencyCycle { .. }),
            ..
        }
    )));
    assert!(world.dependency_graph_acyclic());
}
