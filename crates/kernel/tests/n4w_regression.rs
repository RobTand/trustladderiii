use tl3_kernel::*;

fn send(world: &mut World, event: Event) {
    let StepResult { world: next, .. } = step(world.clone(), event);
    assert!(next.valid(), "invalid after {event:?}: {:?}\n{next:#?}", next.check());
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
    let target = DependencyTarget {
        kind: PrerequisiteKind::Issue,
        number: 7,
        request: None,
    };
    send(
        world,
        Event::Fact {
            node,
            fact: Fact::Dependency { target, status },
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

#[test]
fn n4w_continue_into_auditing_after_revoke() {
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
    eprintln!("W1 ingesting: phase={:?} credit={:?}", world.nodes[0].phase, world.nodes[0].credit);
    assert_eq!(world.nodes[0].phase, Phase::Ingesting);

    // Declare and deliver while still Ingesting: the owner is delivered.
    declare(&mut world, node, DependencyStatus::Pending);
    declare(&mut world, node, DependencyStatus::Delivered);
    eprintln!("W2 delivered: row={:?} credit={:?}", row_status(&world), world.nodes[0].credit);
    assert_eq!(row_status(&world), Some(DependencyStatus::Delivered));

    // Ingest: funded Auditing owner, credit not withheld.
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
    eprintln!(
        "W3 auditing: phase={:?} effect={:?} credit={:?} balance={}",
        n.phase,
        n.effect,
        n.credit,
        n.credit.balance()
    );
    assert_eq!(n.phase, Phase::Auditing);
    assert!(n.credit.ready && !n.credit.withheld && n.credit.balance() >= 2);

    // EffectFailed ask: AwaitingRob with an Auditing continuation, credit kept.
    let effect = n.effect.expect("W4 precondition: Auditing has an in-flight effect to fail");
    send(
        &mut world,
        Event::Ack {
            node,
            seq: effect.seq,
            ack: Ack::Permanent,
        },
    );
    let n = world.nodes[0];
    eprintln!(
        "W4 asked: phase={:?} question={:?} credit={:?} balance={}",
        n.phase,
        n.question,
        n.credit,
        n.credit.balance()
    );
    assert_eq!(n.phase, Phase::AwaitingRob);
    let q = n.question.expect("W4: question asked");
    assert_eq!(q.kind, QKind::EffectFailed);
    assert_eq!(q.cont, Cont::Auditing);
    assert!(!n.credit.withheld && n.credit.balance() >= 2);

    // Pending revoke of the existing Delivered row while AwaitingRob.
    declare(&mut world, node, DependencyStatus::Pending);
    let n = world.nodes[0];
    eprintln!(
        "W5 revoked: row={:?} phase={:?} credit={:?} check={:?}",
        row_status(&world),
        n.phase,
        n.credit,
        world.check()
    );
    assert_eq!(row_status(&world), Some(DependencyStatus::Pending));
    assert_eq!(world.check(), Ok(()), "accepted(before)");

    // Answer Continue: back into Auditing.
    let id = n.next_command_id().expect("command id");
    let event = Event::Command {
        node,
        id,
        command: Command::Answer {
            question: q.id,
            answer: Answer::Continue,
        },
    };
    let (settled, _, trace) = step_traced(world.clone(), event);
    let after = &trace[0];
    let a = after.nodes[0];
    eprintln!(
        "W6 after apply: phase={:?} credit={:?} balance={} row={:?} check={:?}",
        a.phase,
        a.credit,
        a.credit.balance(),
        row_status(after),
        after.check()
    );
    eprintln!(
        "W7 after settle: phase={:?} credit={:?} check={:?} valid={}",
        settled.nodes[0].phase,
        settled.nodes[0].credit,
        settled.check(),
        settled.valid()
    );
    assert_eq!(a.phase, Phase::Auditing);
    assert_eq!(after.check(), Ok(()), "Continue must preserve the checker before settle");
    assert!(after.valid());
    assert_eq!(settled.check(), Ok(()), "Continue must preserve the checker after settle");
    assert!(settled.valid());
}
