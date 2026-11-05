// #805 exact selector inputs and the public theorem over real step.
#[cfg(verus_keep_ghost)]
verus! {
/// State-table prerequisite wait / #805: a real constructor supplies a settled
/// open world in scope, whose selected event is prerequisite delivery. This is
/// a reachability witness, not a finite bound on the universal theorem.
fn progress_scope_witness() -> (world: World)
    ensures progress_scope(world),
        dependencies::delivery_input(world, 0, DependencyTarget {
            kind: PrerequisiteKind::Issue, number: 1, request: None }),
{
    let mut node = Node::new(RepoId(0));
    node.phase = Phase::Unplanned;
    node.credit.ready = true;
    node.credit.start = 3;
    node.credit.withheld = true;
    node.planning_started = false;
    let mut nodes = Vec::new();
    nodes.push(node);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    let target = DependencyTarget { kind: PrerequisiteKind::Issue, number: 1, request: None };
    let mut dependencies = Vec::new();
    dependencies.push(Dependency { node: NodeId(0), target, status: DependencyStatus::Pending, rejected: false });
    let world = World { nodes, gates, dependencies, pool_capacity: 1,
        next_ticket: 0, next_question: 0, file_sets: Vec::new(), fail_fast: false };
    proof {
        reveal(initial_node);
        reveal(progress_scope);
        reveal(world_checked);
        reveal(settled_world);
        reveal(storage_ordered);
        reveal(world_headroom);
        reveal(open_prefix);
        reveal(fresh_commands);
        reveal(dependencies::delivery_input);
        reveal(footprint::ordered_table);
        dependencies::indexable_definition(world);
        dependencies::pending_blocks(world, 0);
        footprint::inline_is_valid(world, 0);
    }
    world
}

pub closed spec fn fresh_commands(w: World) -> bool {
    w.nodes@.len() <= u64::MAX as int
        && forall|j: int| 0 <= j < w.nodes@.len() ==> w.nodes@[j].last_cmd < u64::MAX
}

pub closed spec fn witness_selection(w: World, result: Option<Event>) -> bool {
    match result {
        Some(event) => exists|j: int| 0 <= j < w.nodes@.len() && chosen_for_node(w, j, event),
        None => forall|j: int| 0 <= j < w.nodes@.len() ==> !outside_available(w, j),
    }
}

spec fn outside_available(w: World, i: int) -> bool {
    let n = w.nodes@[i];
    !(n.phase is Closed) && (
        (n.paused && !(n.phase is Stopping)) || n.effect.is_some()
            || n.waiting_incident || n.question.is_some()
            || (n.phase == Phase::Unplanned && !dependencies::delivered(w.dependencies@, NodeId(i as u64)))
            || matches!(current_item(n), Some(Item::Await | Item::Merge)))
}

spec fn pool_line(n: Node) -> bool { !(n.phase is Ready) && n.ticket.is_some() }

spec fn pool_order(n: Node) -> nat {
    priority_number(n.priority) * (u64::MAX as nat + 1) + n.ticket.unwrap() as nat
}

spec fn successful_answer(kind: QKind) -> Answer {
    match kind {
        QKind::AuditDisagreement | QKind::EffectFailed => Answer::Continue,
        QKind::NeedsSplitting => Answer::Stop,
        QKind::CantMeet => Answer::TryAgain,
        QKind::OutOfCredit => Answer::AddCredit(1),
        QKind::CleanupFailed => Answer::MarkHandled,
    }
}

spec fn successful_reply(n: Node) -> Option<Reply> {
    let head = match n.head { Some(h) => h, None => HeadId(0) };
    match n.phase {
        Phase::Ingesting => Some(Reply::IngestDone { size: 1 }),
        Phase::Auditing => Some(Reply::AuditAgrees),
        Phase::Planning { must_split: true } => Some(Reply::PlanSplit),
        Phase::Planning { must_split: false } => Some(Reply::PlanLeaf {
            footprint: if !footprint::footprint_empty(n.footprint) { n.footprint }
                else { footprint::inline_footprint(1) }, fits: true }),
        Phase::Making => Some(Reply::MakerDone { head }),
        Phase::Checking => Some(Reply::ChecksDone { report: true, pass: true, in_footprint: true }),
        Phase::Reviewing => Some(Reply::Approved { head }),
        Phase::Rebasing => Some(Reply::RebaseClean { head }),
        _ => None,
    }
}

spec fn chosen_for_node(w: World, i: int, event: Event) -> bool {
    let n = w.nodes@[i];
    let node = NodeId(i as u64);
    let id = (n.last_cmd as int + 1) as u64;
    if n.phase is Closed { false }
    else if n.paused && !(n.phase is Stopping) {
        event == Event::Command { node, id, command: Command::Resume }
    } else if let Some(e) = n.effect {
        event == Event::Ack { node, seq: e.seq, ack: Ack::Confirmed }
    } else if n.waiting_incident {
        event == Event::Fact { node, fact: Fact::IncidentCleared { by_rob: true } }
    } else if let Some(q) = n.question {
        event == Event::Command { node, id, command: Command::Answer {
            question: q.id, answer: successful_answer(q.kind) } }
    } else if n.phase == Phase::Unplanned && !dependencies::delivered(w.dependencies@, node) {
        exists|k: int| 0 <= k < w.dependencies@.len()
            && w.dependencies@[k].node == node
            && (w.dependencies@[k].rejected || w.dependencies@[k].status != DependencyStatus::Delivered)
            && if w.dependencies@[k].rejected || w.dependencies@[k].status == DependencyStatus::Abandoned {
                event == Event::Command { node, id, command: Command::ChangeRequest }
            } else {
                event == Event::Fact { node, fact: Fact::Dependency {
                    target: w.dependencies@[k].target, status: DependencyStatus::Delivered } }
            }
    } else { match current_item(n) {
        Some(Item::Await) => match successful_reply(n) {
            Some(reply) => event == Event::Reply { node, generation: n.generation, reply },
            None => false,
        },
        Some(Item::Merge) => event == Event::Fact { node, fact: Fact::PrMerged {
            head: match n.head { Some(h) => h, None => HeadId(0) }, checks_passed: true } },
        _ => false,
    } }
}

spec fn ordinary_await(n: Node) -> bool {
    current_item(n) == Some(Item::Await) && !n.paused && n.effect.is_none()
        && !n.waiting_incident && n.question.is_none()
}

spec fn successful_reply_input(n: Node, reply: Reply) -> bool {
    ordinary_await(n) && successful_reply(n) == Some(reply)
}

spec fn answer_progress_input(n: Node, id: u64, answer: Answer) -> bool {
    (!n.paused || n.phase is Stopping) && !n.waiting_incident && n.effect.is_none()
        && match n.question {
            Some(q) => q.id == id && answer == successful_answer(q.kind),
            None => false,
        }
}

spec fn command_progress_input(n: Node, command: Command) -> bool {
    (n.paused && !(n.phase is Stopping || n.phase is Closed) && command == Command::Resume)
        || ((!n.paused || n.phase is Stopping) && !n.waiting_incident && n.effect.is_none() && match command {
            Command::Answer { question, answer } => answer_progress_input(n, question, answer),
            Command::ChangeRequest => n.phase == Phase::Unplanned && n.question.is_none(),
            _ => false,
        })
}

pub closed spec fn progress_input(w: World, event: Event) -> bool {
    world_checked(w) && fresh_commands(w) && witness_event(w, event)
}

spec fn witness_event(w: World, event: Event) -> bool {
    exists|j: int| 0 <= j < w.nodes@.len() && chosen_for_node(w, j, event)
}

pub closed spec fn progress_scope(w: World) -> bool {
    world_checked(w) && settled_world(w) && world_headroom(w) && fresh_commands(w)
        && storage_ordered(w)
        && exists|i: int| 0 <= i < w.nodes@.len() && !(w.nodes@[i].phase is Closed)
}

fn kernel_progress(world: World) -> (result: (World, Vec<Out>))
    requires progress_scope(world),
    ensures progress_measure(result.0) < progress_measure(world),
        step_budget(world, result.0),
{
    proof {
        reveal(progress_scope);
        reveal(progress_input);
        reveal(storage_ordered);
        checked_reserves(world);
        outside_branch_exists(world);
    }
    let event = world.witness();
    proof { reveal(witness_selection); }
    step(world, event.unwrap())
}

spec fn fact_progress_input(w: World, i: int, fact: Fact) -> bool {
    world_checked(w) && chosen_for_node(w, i, Event::Fact { node: NodeId(i as u64), fact })
}
}
