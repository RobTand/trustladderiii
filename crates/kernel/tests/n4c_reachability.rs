use tl3_kernel::*;

fn send(world: &mut World, event: Event) {
    let StepResult { world: next, .. } = step(world.clone(), event);
    assert_eq!(next.check(), Ok(()), "checker rejected after {event:?}");
    *world = next;
}

fn waiting_ingest_failure() -> World {
    let mut world = World::new(&[1], 1);
    assert_eq!(world.check(), Ok(()));
    send(
        &mut world,
        Event::NewRequest {
            repo: RepoId(0),
            priority: Priority::Unranked,
        },
    );
    send(
        &mut world,
        Event::Fact {
            node: NodeId(0),
            fact: Fact::Dependency {
                target: DependencyTarget {
                    kind: PrerequisiteKind::Issue,
                    number: 7,
                    request: None,
                },
                status: DependencyStatus::Pending,
            },
        },
    );
    let effect = world.nodes[0].effect.expect("initial ingest effect");
    send(
        &mut world,
        Event::Ack {
            node: NodeId(0),
            seq: effect.seq,
            ack: Ack::Permanent,
        },
    );
    let n = world.nodes[0];
    assert_eq!(n.phase, Phase::AwaitingRob);
    assert!(!n.planning_started);
    assert!(!n.credit.ready);
    assert!(!n.credit.withheld);
    assert_eq!(n.credit.balance(), 0);
    assert!(!world.dependencies_delivered(NodeId(0)));
    assert!(world.dependencies().iter().any(|d| {
        d.node == NodeId(0) && !d.rejected && d.status == DependencyStatus::Pending
    }));
    let q = n.question.expect("failed ingest asks Rob");
    assert_eq!(q.kind, QKind::EffectFailed);
    assert_eq!(q.cont, Cont::Ingesting);
    assert_eq!(q.cost, 0);
    assert_eq!(world.check(), Ok(()), "accepted before answer");
    world
}

#[test]
fn public_ingest_failure_ignores_add_credit_and_stays_accepted() {
    let before = waiting_ingest_failure();
    let n = before.nodes[0];
    let q = n.question.unwrap();
    let id = n.next_command_id().expect("command id");
    let (after, _, trace) = step_traced(
        before,
        Event::Command {
            node: NodeId(0),
            id,
            command: Command::Answer {
                question: q.id,
                answer: Answer::AddCredit(2),
            },
        },
    );
    assert!(!trace.is_empty());
    for snapshot in trace.iter().chain(std::iter::once(&after)) {
        assert_eq!(snapshot.check(), Ok(()), "accepted after AddCredit(2)");
        assert_eq!(snapshot.nodes[0].phase, Phase::AwaitingRob);
        assert_eq!(snapshot.nodes[0].question, Some(q));
        assert_eq!(snapshot.nodes[0].credit, n.credit);
        assert_eq!(snapshot.nodes[0].last_cmd, id);
        assert!(!snapshot.dependencies_delivered(NodeId(0)));
    }
    println!("N4C_PROBE public EffectFailed/Ingesting: before=Ok after_apply=Ok after_settle=Ok AddCredit(2)=ignored");
}

#[test]
fn alleged_out_of_credit_ingesting_before_is_rejected() {
    let mut world = waiting_ingest_failure();
    let q = world.nodes[0].question.as_mut().unwrap();
    q.kind = QKind::OutOfCredit;
    q.cost = 1;
    assert!(!world.nodes[0].credit.ready);
    assert_eq!(world.check(), Err(Broken::Question));
    println!("N4C_PROBE OutOfCredit/Ingesting ready=false: before=Err(Question)");
}

#[test]
fn out_of_credit_ingesting_is_rejected_even_with_ready_credit() {
    let mut world = waiting_ingest_failure();
    // Establish that ready credit itself is accepted in the waiting state.
    world.nodes[0].credit.ready = true;
    world.nodes[0].credit.start = 2;
    assert_eq!(world.check(), Ok(()));
    let q = world.nodes[0].question.as_mut().unwrap();
    q.kind = QKind::OutOfCredit;
    q.cost = 1;
    assert_eq!(world.check(), Err(Broken::Question));
    println!("N4C_PROBE OutOfCredit/Ingesting ready=true: before=Err(Question)");
}
