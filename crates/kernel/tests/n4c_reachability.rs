use tl3_kernel::*;

// Executable mirror of owner_waiting_ok at main 4cd55642 dependencies.rs:308.
// The production predicate is ghost-only and private to the kernel crate.
fn owner_waiting_ok_mirror(n: Node) -> bool {
    let waits = n.question.is_none_or(|q| {
        matches!(
            q.cont,
            Cont::Ingesting | Cont::Auditing | Cont::Unplanned | Cont::Stop(_) | Cont::Nowhere
        )
    });
    let early = matches!(n.phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned);
    let blocked = matches!(
        n.phase,
        Phase::Ingesting
            | Phase::Auditing
            | Phase::Unplanned
            | Phase::AwaitingRob
            | Phase::Stopping(_)
            | Phase::Closed(_)
    );
    !n.planning_started
        && waits
        && blocked
        && (!early || n.credit.balance() == 0)
        && (n.phase != Phase::Unplanned
            || (!n.holds && !n.pool && n.ticket.is_none() && n.run.is_none()))
}

fn print_rules(label: &str, world: &World, delivered_before: bool) -> (bool, bool) {
    // World::check's ensures equates Ok with checking::accepted (checking.rs:175).
    let accepted = world.check().is_ok();
    let delivered = world.dependencies_delivered(NodeId(0));
    let owner_waiting = owner_waiting_ok_mirror(world.nodes[0]);
    let dependency_safety = delivered_before || owner_waiting;
    println!(
        "N4C_RULES {label}: checking_accepted_via_check={accepted} prerequisites_delivered={delivered} owner_waiting_ok_mirror={owner_waiting} before_delivered_or_after_waiting={dependency_safety}"
    );
    (accepted, dependency_safety)
}

fn send(world: &mut World, event: Event) {
    let StepResult { world: next, .. } = step(world.clone(), event);
    assert_eq!(next.check(), Ok(()), "checker rejected after {event:?}");
    println!("N4C_PUBLIC_STEP {event:?}: checking_accepted_via_check=true");
    *world = next;
}

fn waiting_ingest_failure(with_dependency: bool) -> World {
    let mut world = World::new(&[1], 1);
    assert_eq!(world.check(), Ok(()));
    send(
        &mut world,
        Event::NewRequest {
            repo: RepoId(0),
            priority: Priority::Unranked,
        },
    );
    if with_dependency {
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
    }
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
    assert_eq!(world.dependencies_delivered(NodeId(0)), !with_dependency);
    assert_eq!(world.dependencies().len(), usize::from(with_dependency));
    if with_dependency {
        assert!(world.dependencies().iter().any(|d| {
            d.node == NodeId(0) && !d.rejected && d.status == DependencyStatus::Pending
        }));
    }
    let q = n.question.expect("failed ingest asks Rob");
    assert_eq!(q.kind, QKind::EffectFailed);
    assert_eq!(q.cont, Cont::Ingesting);
    assert_eq!(q.cost, 0);
    assert_eq!(world.check(), Ok(()), "accepted before answer");
    assert_eq!(
        print_rules("public before answer", &world, !with_dependency),
        (true, true)
    );
    world
}

fn public_ingest_failure_add_credit(with_dependency: bool) {
    let before = waiting_ingest_failure(with_dependency);
    let delivered_before = before.dependencies_delivered(NodeId(0));
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
        assert_eq!(snapshot.dependencies_delivered(NodeId(0)), delivered_before);
        assert_eq!(
            print_rules("public after AddCredit(2)", snapshot, delivered_before),
            (true, true)
        );
    }
    println!("N4C_PROBE public EffectFailed/Ingesting dependency_rows={}: before=Ok after_apply=Ok after_settle=Ok AddCredit(2)=ignored", after.dependencies().len());
}

#[test]
fn public_dependent_ingest_failure_ignores_add_credit_and_stays_accepted() {
    public_ingest_failure_add_credit(true);
}

#[test]
fn public_ingest_continuation_can_be_not_ready_with_no_dependency_rows() {
    public_ingest_failure_add_credit(false);
}

#[test]
fn hand_built_out_of_credit_ingesting_before_is_rejected() {
    let mut world = waiting_ingest_failure(true);
    let q = world.nodes[0].question.as_mut().unwrap();
    q.kind = QKind::OutOfCredit;
    q.cost = 1;
    assert!(!world.nodes[0].credit.ready);
    assert_eq!(world.check(), Err(Broken::Question));
    assert_eq!(
        print_rules("HAND_BUILT OutOfCredit/Ingesting ready=false", &world, false),
        (false, true)
    );
    println!("N4C_PROBE HAND_BUILT OutOfCredit/Ingesting ready=false: before=Err(Question), not a reachable witness");
}

#[test]
fn hand_built_out_of_credit_ingesting_is_rejected_even_with_ready_credit() {
    let mut world = waiting_ingest_failure(true);
    // Establish that ready credit itself is accepted in the waiting state.
    world.nodes[0].credit.ready = true;
    world.nodes[0].credit.start = 2;
    assert_eq!(world.check(), Ok(()));
    let q = world.nodes[0].question.as_mut().unwrap();
    q.kind = QKind::OutOfCredit;
    q.cost = 1;
    assert_eq!(world.check(), Err(Broken::Question));
    assert_eq!(
        print_rules("HAND_BUILT OutOfCredit/Ingesting ready=true", &world, false),
        (false, true)
    );
    println!("N4C_PROBE HAND_BUILT OutOfCredit/Ingesting ready=true: before=Err(Question), not a reachable witness");
}

#[test]
fn public_delivered_owner_need_not_remain_waiting_after_continue() {
    let mut world = World::new(&[3], 3);
    assert_eq!(world.check(), Ok(()));
    send(
        &mut world,
        Event::NewRequest {
            repo: RepoId(0),
            priority: Priority::Unranked,
        },
    );
    for _ in 0..32 {
        let Some(effect) = world.nodes[0].effect else {
            break;
        };
        send(
            &mut world,
            Event::Ack {
                node: NodeId(0),
                seq: effect.seq,
                ack: Ack::Confirmed,
            },
        );
    }
    assert!(world.nodes[0].effect.is_none());
    let generation = world.nodes[0].generation;
    send(
        &mut world,
        Event::Reply {
            node: NodeId(0),
            generation,
            reply: Reply::IngestDone { size: 1 },
        },
    );
    assert_eq!(world.nodes[0].phase, Phase::Auditing);
    assert!(world.nodes[0].credit.ready && world.nodes[0].credit.balance() > 0);
    let effect = world.nodes[0].effect.expect("audit effect");
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
    let q = n.question.expect("failed audit asks Rob");
    assert_eq!(q.kind, QKind::EffectFailed);
    assert_eq!(q.cont, Cont::Auditing);
    assert!(world.dependencies().is_empty());
    assert!(world.dependencies_delivered(NodeId(0)));
    assert!(owner_waiting_ok_mirror(n));
    assert_eq!(
        print_rules("public no-row before Continue", &world, true),
        (true, true)
    );
    let (after, _, trace) = step_traced(
        world,
        Event::Command {
            node: NodeId(0),
            id: n.next_command_id().expect("command id"),
            command: Command::Answer {
                question: q.id,
                answer: Answer::Continue,
            },
        },
    );
    let immediate = &trace[0];
    assert_eq!(immediate.nodes[0].phase, Phase::Auditing);
    assert!(immediate.nodes[0].credit.balance() > 0);
    assert!(!owner_waiting_ok_mirror(immediate.nodes[0]));
    for snapshot in trace.iter().chain(std::iter::once(&after)) {
        assert_eq!(
            print_rules("public no-row after Continue", snapshot, true),
            (true, true)
        );
    }
    println!("N4C_PROBE public no-row Continue: owner_waiting_ok true->false, checking_accepted true->true, dependency rule true->true; original N4c is stronger than R28 needs");
}
