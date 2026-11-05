// #805 exact selector inputs and the public theorem over real step.
#[cfg(verus_keep_ghost)]
verus! {
/// State-table counter headroom: each selected steering command has a fresh ID.
pub closed spec fn fresh_commands(w: World) -> bool {
    w.nodes@.len() <= u64::MAX as int
        && forall|j: int| 0 <= j < w.nodes@.len() ==> w.nodes@[j].last_cmd < u64::MAX
}

/// Q30/Q39: an actual selector result identifies an available outside branch.
pub closed spec fn witness_selection(w: World, result: Option<Event>) -> bool {
    match result {
        Some(event) => exists|j: int| 0 <= j < w.nodes@.len() && chosen_for_node(w, j, event),
        None => forall|j: int| 0 <= j < w.nodes@.len() ==> !outside_available(w, j),
    }
}

/// Q30/Q39: outside inputs for phases blocked on replies, facts, or Rob.
spec fn outside_available(w: World, i: int) -> bool {
    let n = w.nodes@[i];
    !(n.phase is Closed) && (
        (n.paused && !(n.phase is Stopping)) || n.effect.is_some()
            || n.waiting_incident || n.question.is_some()
            || (n.phase == Phase::Unplanned && !dependencies::delivered(w.dependencies@, NodeId(i as u64)))
            || matches!(current_item(n), Some(Item::Await | Item::Merge)))
}

/// Q39: a non-Ready ticket waits for the agent pool.
spec fn pool_line(n: Node) -> bool { !(n.phase is Ready) && n.ticket.is_some() }

/// Q39: priority then ticket, encoded without machine arithmetic overflow.
#[verifier::opaque]
spec fn pool_order(n: Node) -> nat {
    priority_number(n.priority) * (u64::MAX as nat + 1) + n.ticket.unwrap() as nat
}

/// State-table question rows: the successful answer for each question kind.
spec fn successful_answer(kind: QKind) -> Answer {
    match kind {
        QKind::AuditDisagreement | QKind::EffectFailed => Answer::Continue,
        QKind::NeedsSplitting => Answer::Stop,
        QKind::CantMeet => Answer::TryAgain,
        QKind::OutOfCredit => Answer::AddCredit(1),
        QKind::CleanupFailed => Answer::MarkHandled,
    }
}

/// State-table reply rows: exact successful payload chosen by World::witness.
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

/// Q30/Q39: exact ordered branches and event keys of World::witness.
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

/// State-table reply rows: an unpaused, unobstructed Await agenda item.
spec fn ordinary_await(n: Node) -> bool {
    current_item(n) == Some(Item::Await) && !n.paused && n.effect.is_none()
        && !n.waiting_incident && n.question.is_none()
}

/// State-table reply rows: the selector payload at an unobstructed Await.
spec fn successful_reply_input(n: Node, reply: Reply) -> bool {
    ordinary_await(n) && successful_reply(n) == Some(reply)
}

/// State-table question rows: a current successful answer has no prior obstruction.
spec fn answer_progress_input(n: Node, id: u64, answer: Answer) -> bool {
    (!n.paused || n.phase is Stopping) && !n.waiting_incident && n.effect.is_none()
        && match n.question {
            Some(q) => q.id == id && answer == successful_answer(q.kind),
            None => false,
        }
}

/// State-table steering rows: resume, answer, or replace the blocked request.
spec fn command_progress_input(n: Node, command: Command) -> bool {
    (n.paused && !(n.phase is Stopping || n.phase is Closed) && command == Command::Resume)
        || ((!n.paused || n.phase is Stopping) && !n.waiting_incident && n.effect.is_none() && match command {
            Command::Answer { question, answer } => answer_progress_input(n, question, answer),
            Command::ChangeRequest => n.phase == Phase::Unplanned && n.question.is_none(),
            _ => false,
        })
}

/// Q30/Q39 and counter headroom: exact selected input to the real step.
pub closed spec fn progress_input(w: World, event: Event) -> bool {
    world_checked(w) && fresh_commands(w) && witness_event(w, event)
}

/// Q30/Q39: one actual open node supplies the selected event.
spec fn witness_event(w: World, event: Event) -> bool {
    exists|j: int| 0 <= j < w.nodes@.len() && chosen_for_node(w, j, event)
}

/// Q30/Q39 and counter headroom: a consistent, settled open world in enforced scope.
pub closed spec fn progress_scope(w: World) -> bool {
    world_checked(w) && settled_world(w) && world_headroom(w) && fresh_commands(w)
        && storage_ordered(w)
        && exists|i: int| 0 <= i < w.nodes@.len() && !(w.nodes@[i].phase is Closed)
}

/// State-table prerequisite, merge, and incident rows: the exact selected fact.
#[verifier::opaque]
spec fn fact_progress_input(w: World, i: int, fact: Fact) -> bool {
    world_checked(w) && chosen_for_node(w, i, Event::Fact { node: NodeId(i as u64), fact })
}
}
