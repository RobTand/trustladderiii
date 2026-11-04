// Included in the kernel module so proofs use the production world's storage.
#[cfg(verus_keep_ghost)]
verus! {
/// #771: checked steering construction can represent the next command for
/// every node. The raw kernel exhaustion characterization is outside this scope.
pub closed spec fn fresh_commands(w: World) -> bool {
    w.nodes@.len() <= u64::MAX as int
        && forall|j: int| 0 <= j < w.nodes@.len() ==> w.nodes@[j].last_cmd < u64::MAX
}

/// #771: expose the real selector's result without exposing private storage.
pub closed spec fn witness_selection(w: World, result: Option<Event>) -> bool {
    match result {
        Some(event) => exists|j: int| 0 <= j < w.nodes@.len() && chosen_for_node(w, j, event),
        None => forall|j: int| 0 <= j < w.nodes@.len() ==> !outside_available(w, j),
    }
}

/// Q30/Q39 / #771: these are exactly the branches considered by witness.
/// Availability says nothing about the event's effect on the rank; that is a
/// separate obligation against apply and the actual event handlers.
spec fn outside_available(w: World, i: int) -> bool {
    let n = w.nodes@[i];
    !(n.phase is Closed) && (
        (n.paused && !(n.phase is Stopping)) || n.effect.is_some()
            || n.waiting_incident || n.question.is_some()
            || (n.phase == Phase::Unplanned && !dependencies::delivered(w.dependencies@, NodeId(i as u64)))
            || matches!(current_item(n), Some(Item::Await | Item::Merge)))
}

/// #771: a finite sum of zero resource counts is zero.
proof fn potential_zero(nodes: Seq<Node>, end: nat, potential: spec_fn(Node) -> nat)
    requires end <= nodes.len(), forall|j: int| 0 <= j < end ==> potential(nodes[j]) == 0,
    ensures potential_prefix(nodes, end, potential) == 0,
    decreases end,
{
    if end > 0 { potential_zero(nodes, (end - 1) as nat, potential); }
}

/// Q30/Q39 / #771: an occupied pool leads to an outside reply or a cleanup
/// move. This precedes the slot argument because a Making node can hold a slot
/// while waiting to start a run.
proof fn no_outside_no_pool(w: World)
    requires world_checked(w), settled_world(w),
        forall|j: int| 0 <= j < w.nodes@.len() ==> !outside_available(w, j),
    ensures forall|j: int| 0 <= j < w.nodes@.len() ==> !w.nodes@[j].pool,
        pool_users(w.nodes@, w.nodes@.len()) == 0,
{
    reveal(world_checked);
    reveal(settled_world);
    assert forall|j: int| 0 <= j < w.nodes@.len() implies !w.nodes@[j].pool by {
        let n = w.nodes@[j];
        if n.pool {
            assert(n.run.is_some());
            assert(n.phase is Stopping && n.pos == 0);
            assert(movable(w, j));
        }
    }
    potential_zero(w.nodes@, w.nodes@.len(), |n: Node| if n.pool { 1nat } else { 0nat });
}

spec fn pool_line(n: Node) -> bool { !(n.phase is Ready) && n.ticket.is_some() }
spec fn pool_order(n: Node) -> nat {
    priority_number(n.priority) * (u64::MAX as nat + 1) + n.ticket.unwrap() as nat
}

/// Q39 / #771: the mathematical key preserves the real priority/ticket order.
proof fn pool_order_strict(a: Node, b: Node)
    requires a.ticket.is_some(), b.ticket.is_some(), pool_precedes(a, b),
    ensures pool_order(a) < pool_order(b),
{
    match a.priority { Priority::P0 => {}, Priority::P1 => {}, Priority::P2 => {},
        Priority::P3 => {}, Priority::Unranked => {} }
    match b.priority { Priority::P0 => {}, Priority::P1 => {}, Priority::P2 => {},
        Priority::P3 => {}, Priority::Unranked => {} }
}

/// Q39 / #771: in a finite pool line, the best priority and oldest ticket
/// actually moves once the occupied-pool obstruction has been discharged.
proof fn no_outside_no_pool_line(w: World)
    requires world_checked(w), settled_world(w),
        forall|j: int| 0 <= j < w.nodes@.len() ==> !outside_available(w, j),
    ensures forall|j: int| 0 <= j < w.nodes@.len() ==> !pool_line(w.nodes@[j]),
{
    no_outside_no_pool(w);
    reveal(world_checked);
    reveal(settled_world);
    if exists|j: int| 0 <= j < w.nodes@.len() && pool_line(w.nodes@[j]) {
        finite_minimum(w.nodes@, w.nodes@.len(), |n: Node| pool_line(n), |n: Node| pool_order(n));
        let i = choose|i: int| 0 <= i < w.nodes@.len() && pool_line(w.nodes@[i])
            && forall|j: int| 0 <= j < w.nodes@.len() && pool_line(w.nodes@[j])
                ==> pool_order(w.nodes@[i]) <= pool_order(w.nodes@[j]);
        let n = w.nodes@[i];
        assert forall|j: int| 0 <= j < w.nodes@.len() implies !pool_precedes(w.nodes@[j], n) by {
            if pool_precedes(w.nodes@[j], n) {
                pool_order_strict(w.nodes@[j], n);
            }
        }
        assert(oldest_waiter(w, i));
        assert(current_item(n) is Some);
        assert(matches!(current_item(n), Some(Item::Start(_))));
        assert(movable(w, i));
        assert(false);
    }
}

/// Q30/Q48 / #771: a slot holder has a reply, merge, effect or cleanup move,
/// or waits for the pool. Therefore the preceding two lemmas remove all holders.
proof fn no_outside_no_holders(w: World)
    requires world_checked(w), settled_world(w),
        forall|j: int| 0 <= j < w.nodes@.len() ==> !outside_available(w, j),
    ensures forall|j: int| 0 <= j < w.nodes@.len() ==> !w.nodes@[j].holds,
{
    no_outside_no_pool_line(w);
    reveal(world_checked);
    reveal(settled_world);
    assert forall|j: int| 0 <= j < w.nodes@.len() implies !w.nodes@[j].holds by {
        let n = w.nodes@[j];
        if n.holds {
            match n.phase {
                Phase::Making | Phase::Checking | Phase::Reviewing | Phase::Publishing
                    | Phase::Queued | Phase::Rebasing | Phase::Stopping(_) => {},
                _ => {},
            }
            assert(movable(w, j));
        }
    }
}

/// Q30/Q48 / #771: with no holders, the oldest Ready ticket can take its slot.
/// Selecting a minimum over every Ready node also makes it oldest in its repo.
proof fn no_outside_no_ready(w: World)
    requires world_checked(w), settled_world(w),
        forall|j: int| 0 <= j < w.nodes@.len() ==> !outside_available(w, j),
    ensures forall|j: int| 0 <= j < w.nodes@.len() ==> !(w.nodes@[j].phase is Ready),
{
    no_outside_no_holders(w);
    reveal(world_checked);
    reveal(settled_world);
    assert forall|j: int| 0 <= j < w.nodes@.len() && w.nodes@[j].phase is Ready
        implies w.nodes@[j].ticket.is_some() by {
        if w.nodes@[j].ticket.is_none() { assert(movable(w, j)); }
    }
    if exists|j: int| 0 <= j < w.nodes@.len() && w.nodes@[j].phase is Ready {
        finite_minimum(w.nodes@, w.nodes@.len(), |n: Node| n.phase is Ready,
            |n: Node| n.ticket.unwrap() as nat);
        let i = choose|i: int| 0 <= i < w.nodes@.len() && w.nodes@[i].phase is Ready
            && forall|j: int| 0 <= j < w.nodes@.len() && w.nodes@[j].phase is Ready
                ==> w.nodes@[i].ticket.unwrap() <= w.nodes@[j].ticket.unwrap();
        assert(oldest_waiter(w, i));
        assert(movable(w, i));
        assert(false);
    }
}

/// Q30/Q39 / #771: every finite, consistent, settled open world has a branch
/// the real outside-witness selector can use. This proof follows pool and slot
/// obstructions in order. It assumes no fairness and proves no starvation bound.
proof fn outside_branch_exists(w: World)
    requires world_checked(w), settled_world(w),
        exists|i: int| 0 <= i < w.nodes@.len() && !(w.nodes@[i].phase is Closed),
    ensures exists|i: int| 0 <= i < w.nodes@.len() && outside_available(w, i),
{
    reveal(world_checked);
    reveal(settled_world);
    if !(exists|j: int| 0 <= j < w.nodes@.len() && outside_available(w, j)) {
        no_outside_no_pool(w);
        no_outside_no_pool_line(w);
        no_outside_no_ready(w);
        let i = choose|i: int| 0 <= i < w.nodes@.len() && !(w.nodes@[i].phase is Closed);
        let n = w.nodes@[i];
        potential_zero(w.nodes@, w.nodes@.len(), |m: Node|
            if m.repo == n.repo && m.phase is Ready && m.ticket.is_some() { 1nat } else { 0nat });
        match n.phase {
            Phase::Ingesting | Phase::Auditing | Phase::Unplanned | Phase::Planning { .. }
                | Phase::Ready(_) | Phase::Making | Phase::Checking | Phase::Reviewing
                | Phase::Publishing | Phase::Queued | Phase::Rebasing | Phase::AwaitingRob
                | Phase::Stopping(_) | Phase::Closed(_) => {},
        }
        assert(movable(w, i));
        assert(false);
    }
}
}

#[cfg(verus_keep_ghost)]
verus! {
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

/// #771: a postcondition for the real selector, including its branch order.
/// It specifies the chosen input event, without defining its transition.
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
}

#[cfg(verus_keep_ghost)]
verus! {
/// #771: a successful outside reply is selected only after direct blockers.
spec fn ordinary_await(n: Node) -> bool {
    current_item(n) == Some(Item::Await) && !n.paused && n.effect.is_none()
        && !n.waiting_incident && n.question.is_none()
}
spec fn successful_reply_input(n: Node, reply: Reply) -> bool {
    ordinary_await(n) && successful_reply(n) == Some(reply)
}
}

#[cfg(verus_keep_ghost)]
verus! {
/// Question rows / #771: the selector supplies exactly this permitted answer.
spec fn answer_progress_input(n: Node, id: u64, answer: Answer) -> bool {
    (!n.paused || n.phase is Stopping) && !n.waiting_incident && n.effect.is_none()
        && match n.question {
            Some(q) => q.id == id && answer == successful_answer(q.kind),
            None => false,
        }
}

/// Command rows / #771: these are the selector's three command cases.
spec fn command_progress_input(n: Node, command: Command) -> bool {
    (n.paused && !(n.phase is Stopping || n.phase is Closed) && command == Command::Resume)
        || ((!n.paused || n.phase is Stopping) && !n.waiting_incident && n.effect.is_none() && match command {
            Command::Answer { question, answer } => answer_progress_input(n, question, answer),
            Command::ChangeRequest => n.phase == Phase::Unplanned && n.question.is_none(),
            _ => false,
        })
}
}

#[cfg(verus_keep_ghost)]
verus! {
/// #693 / #771: delivery of an existing pending prerequisite is an outside
/// witness. Rejected and abandoned declarations instead select ChangeRequest.
spec fn dependency_progress_input(w: World, i: int, target: DependencyTarget,
    status: DependencyStatus) -> bool {
    !(w.nodes@[i].phase is Closed) && status == DependencyStatus::Delivered
        && exists|k: int| 0 <= k < w.dependencies@.len()
            && w.dependencies@[k].node == NodeId(i as u64) && w.dependencies@[k].target == target
            && !w.dependencies@[k].rejected && w.dependencies@[k].status == DependencyStatus::Pending
}

/// #771: one summand of the production measure's prerequisite count.
spec fn dependency_weight(w: World, j: int) -> nat {
    if w.dependencies@[j].status != DependencyStatus::Delivered
        && !(w.nodes@[w.dependencies@[j].node.0 as int].phase is Closed) { 1nat } else { 0nat }
}

spec fn dependency_total(w: World, end: nat) -> nat
    recommends end <= w.dependencies@.len(),
    decreases end,
{
    if end == 0 { 0 } else { dependency_total(w, (end - 1) as nat) + dependency_weight(w, end as int - 1) }
}

/// #771: the mathematical version of World::measure has no sum overflow.
pub closed spec fn progress_measure(w: World) -> nat {
    node_total(w.nodes@) + dependency_total(w, w.dependencies@.len())
}

/// #771: pointwise nonincrease, including one strict summand, lifts to the sum.
proof fn dependency_sum_compare(before: World, after: World, end: nat)
    requires end <= before.dependencies@.len(), end <= after.dependencies@.len(),
        forall|j: int| 0 <= j < end ==> dependency_weight(after, j) <= dependency_weight(before, j),
    ensures dependency_total(after, end) <= dependency_total(before, end),
        (exists|j: int| 0 <= j < end && dependency_weight(after, j) < dependency_weight(before, j))
            ==> dependency_total(after, end) < dependency_total(before, end),
    decreases end,
{
    if end > 0 { dependency_sum_compare(before, after, (end - 1) as nat); }
}
}

#[cfg(verus_keep_ghost)]
verus! {
/// #771: changing one node strictly lowers the world sum while prerequisite
/// debt cannot grow. Closed is final, including during cleanup settlement.
proof fn lift_node_progress(before: World, after: World, i: int)
    requires 0 <= i < before.nodes@.len(),
        after.nodes@ == before.nodes@.update(i, after.nodes@[i]),
        after.dependencies@ == before.dependencies@, dependencies::edges_indexable(before),
        (before.nodes@[i].phase is Closed ==> after.nodes@[i].phase is Closed),
        distance(after.nodes@[i]) < distance(before.nodes@[i]),
    ensures progress_measure(after) < progress_measure(before),
{
    reveal(progress_measure);
    dependencies::indexable_definition(before);
    sum_update(before.nodes@, i, after.nodes@[i]);
    assert forall|j: int| 0 <= j < before.dependencies@.len() implies
        dependency_weight(after, j) <= dependency_weight(before, j) by {
        let owner = before.dependencies@[j].node.0 as int;
        if owner != i { assert(after.nodes@[owner] == before.nodes@[owner]); }
    }
    dependency_sum_compare(before, after, before.dependencies@.len());
}

/// #771: settle preserves prerequisite rows and never reopens a closed owner.
proof fn settle_measure(before: World, after: World)
    requires settle_chain(before, after),
        dependencies::edges_indexable(before),
        forall|j: int| 0 <= j < before.nodes@.len()
            ==> (before.nodes@[j].phase is Closed ==> after.nodes@[j].phase is Closed),
    ensures progress_measure(after) <= progress_measure(before),
{
    reveal(progress_measure);
    dependencies::indexable_definition(before);
    assert forall|j: int| 0 <= j < before.dependencies@.len() implies
        dependency_weight(after, j) <= dependency_weight(before, j) by {}
    dependency_sum_compare(before, after, before.dependencies@.len());
}
}

#[cfg(verus_keep_ghost)]
verus! {
/// #771: public view of the source-connected outside event premise.
pub closed spec fn progress_input(w: World, event: Event) -> bool {
    world_checked(w) && witness_event(w, event)
}
/// #771: an event returned by the real outside-witness selector.
spec fn witness_event(w: World, event: Event) -> bool {
    exists|j: int| 0 <= j < w.nodes@.len() && chosen_for_node(w, j, event)
}
}

#[cfg(verus_keep_ghost)]
verus! {
/// Q30/Q39 / #771: consistency and settledness plus the enforced arithmetic
/// domain. Node::next_command_id uses checked_add. Controller::translate
/// rejects raw Commands, and steering's command_event supplies the checked ID.
/// Controller::apply checks World::headroom_ok before each event. Exhausting a
/// command ID requires about 2^64 accepted steering commands on one request;
/// the unrestricted raw-kernel counterexample remains outside this scope.
/// Immutable file storage is ordered by World::intern_footprint.
pub closed spec fn progress_scope(w: World) -> bool {
    world_checked(w) && settled_world(w) && world_headroom(w) && fresh_commands(w)
        && storage_ordered(w)
        && exists|i: int| 0 <= i < w.nodes@.len() && !(w.nodes@[i].phase is Closed)
}

/// #771: source-connected progress of the executable step. The rank is the
/// mathematical version of measure(), avoiding overflow of its finite sum.
/// This proves existence of some progress event. Q30/Q39 permit starvation.
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
}
