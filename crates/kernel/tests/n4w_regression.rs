use tl3_kernel::*;

fn send(world: &mut World, event: Event) {
    let StepResult { world: next, .. } = step(world.clone(), event);
    assert!(
        next.valid(),
        "invalid after {event:?}: {:?}\n{next:#?}",
        next.check()
    );
    *world = next;
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

fn declare(world: &mut World, node: NodeId, status: DependencyStatus) {
    send(
        world,
        Event::Fact {
            node,
            fact: Fact::Dependency {
                target: DependencyTarget {
                    kind: PrerequisiteKind::Issue,
                    number: 7,
                    request: None,
                },
                status,
            },
        },
    );
}

fn row_status(world: &World) -> Option<DependencyStatus> {
    world
        .dependencies()
        .iter()
        .find(|d| d.node == NodeId(0) && d.target.number == 7)
        .map(|d| d.status)
}

fn auditing() -> World {
    let node = NodeId(0);
    let mut world = World::new(&[3], 3);
    send(
        &mut world,
        Event::NewRequest {
            repo: RepoId(0),
            priority: Priority::Unranked,
        },
    );
    confirm(&mut world, node);
    assert_eq!(world.nodes[0].phase, Phase::Ingesting);
    declare(&mut world, node, DependencyStatus::Pending);
    declare(&mut world, node, DependencyStatus::Delivered);
    assert_eq!(row_status(&world), Some(DependencyStatus::Delivered));
    confirm(&mut world, node);
    let generation = world.nodes[0].generation;
    send(
        &mut world,
        Event::Reply {
            node,
            generation,
            reply: Reply::IngestDone { size: 1 },
        },
    );
    let n = world.nodes[0];
    assert_eq!(n.phase, Phase::Auditing);
    assert!(n.credit.ready && !n.credit.withheld && n.credit.balance() >= 2);
    world
}

fn continue_after_revoke(mut world: World, continuation: Cont, phase: Phase) {
    let node = NodeId(0);
    let effect = world.nodes[0].effect.expect("early phase has an effect to fail");
    send(
        &mut world,
        Event::Ack {
            node,
            seq: effect.seq,
            ack: Ack::Permanent,
        },
    );
    let n = world.nodes[0];
    assert_eq!(n.phase, Phase::AwaitingRob);
    let q = n.question.expect("EffectFailed question");
    assert_eq!(q.kind, QKind::EffectFailed);
    assert_eq!(q.cont, continuation);
    assert!(!n.credit.withheld && n.credit.balance() >= 2);
    declare(&mut world, node, DependencyStatus::Pending);
    assert_eq!(row_status(&world), Some(DependencyStatus::Pending));
    assert_eq!(world.check(), Ok(()), "accepted before Continue");
    let n = world.nodes[0];
    let event = Event::Command {
        node,
        id: n.next_command_id().expect("command id"),
        command: Command::Answer {
            question: q.id,
            answer: Answer::Continue,
        },
    };
    let (settled, _, trace) = step_traced(world, event);
    let after = &trace[0];
    assert_eq!(after.nodes[0].phase, phase);
    assert_eq!(
        after.check(),
        Ok(()),
        "Continue must preserve the checker before settle"
    );
    assert_eq!(
        settled.check(),
        Ok(()),
        "Continue must preserve the checker after settle"
    );
    assert!(settled.valid());
    assert!(after.nodes[0].credit.withheld);
    assert_eq!(after.nodes[0].credit.balance(), 0);
    assert!(settled.nodes[0].credit.withheld);
    assert_eq!(settled.nodes[0].credit.balance(), 0);
}

#[test]
fn n4w_continue_into_auditing_after_revoke() {
    continue_after_revoke(auditing(), Cont::Auditing, Phase::Auditing);
}

#[test]
fn n4w_continue_into_ingesting_after_first_disagreement_and_revoke() {
    let mut world = auditing();
    let node = NodeId(0);
    confirm(&mut world, node);
    let generation = world.nodes[0].generation;
    send(
        &mut world,
        Event::Reply {
            node,
            generation,
            reply: Reply::AuditDisagrees,
        },
    );
    assert_eq!(world.nodes[0].phase, Phase::Ingesting);
    assert_eq!(world.nodes[0].audit_rounds, 1);
    continue_after_revoke(world, Cont::Ingesting, Phase::Ingesting);
}
