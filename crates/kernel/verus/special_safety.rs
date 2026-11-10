//! K4c: concept lemmas replacing the Kani SPECIAL harnesses, over arbitrary
//! nodes and worlds rather than the harnesses' bounded fixtures.
use super::*;

verus! {
/// Replaces Kani `apply_rebase_progress_gate`. A Rebasing request awaits its
/// rebase reply exactly at agenda position 1, after StartRebase at position 0,
/// so the rebase fuel rule applies only to a reply the request is awaiting.
pub(super) proof fn rebase_await_gate(n: Node)
    requires n.phase == Phase::Rebasing,
    ensures (current_item(n) == Some(Item::Await)) == (n.pos == 1),
        (current_item(n) == Some(Item::StartRebase)) == (n.pos == 0),
        n.pos >= 2 ==> current_item(n).is_none(),
{
}

/// Replaces the order assertion of Kani `pool_grant_is_priority_ordered` for
/// arbitrary worlds. A pool waiter (a non-Ready ticket holder) is first in
/// line exactly when no other waiter has a better priority, or the same
/// priority and an older ticket.
pub(super) proof fn pool_line_order(w: World, i: int)
    requires 0 <= i < w.nodes@.len(), !(w.nodes@[i].phase is Ready),
        w.nodes@[i].ticket.is_some(),
    ensures oldest_waiter(w, i) == forall|j: int| 0 <= j < w.nodes@.len()
        && !(w.nodes@[j].phase is Ready) && w.nodes@[j].ticket.is_some()
        ==> !(priority_number(w.nodes@[j].priority) < priority_number(w.nodes@[i].priority)
            || (w.nodes@[j].priority == w.nodes@[i].priority
                && w.nodes@[j].ticket.unwrap() < w.nodes@[i].ticket.unwrap())),
{
    let me = w.nodes@[i];
    if oldest_waiter(w, i) {
        assert forall|j: int| 0 <= j < w.nodes@.len()
            && !(w.nodes@[j].phase is Ready) && w.nodes@[j].ticket.is_some()
            implies !(priority_number(w.nodes@[j].priority) < priority_number(me.priority)
                || (w.nodes@[j].priority == me.priority
                    && w.nodes@[j].ticket.unwrap() < me.ticket.unwrap())) by {
            assert(!pool_precedes(w.nodes@[j], me));
        }
    } else {
        let j = choose|j: int| 0 <= j < w.nodes@.len() && pool_precedes(w.nodes@[j], me);
        assert(!(w.nodes@[j].phase is Ready) && w.nodes@[j].ticket.is_some());
    }
}

/// Replaces the residual assertion of Kani `gate_planning_w2/w3` (proofs.rs:1355
/// `assert!(!after.nodes[0].holds && !after.nodes[0].pool)`) for arbitrary
/// accepted worlds: an Unplanned request holds no slot and no pool run, and
/// its internal step (admission to Planning, or none) keeps both released.
/// The harness's other assertions are scheduler::advance's Unplanned ensures
/// (changed == waiters < W) and advance_post (rank decrease). Only the real
/// step's advance_post is needed, not its enabling condition (mutant e3d4ff4e).
pub(super) proof fn gate_planning_holds_nothing(w: World, i: int, after: Node,
    ticket_before: u64, ticket_after: u64, changed: bool)
    requires checking::accepted(w), 0 <= i < w.nodes@.len(),
        w.nodes@[i].phase == Phase::Unplanned,
        advance_post(w.nodes@[i], after, ticket_before, ticket_after, changed),
    ensures !w.nodes@[i].holds && !w.nodes@[i].pool,
        !after.holds && !after.pool,
{
    checking::accepted_node(w, i);
    reveal(advance_post);
}
}

mod pool_assembly {
    use super::*;

    verus! {
    /// G20: the real pool step, including the priority and ticket order checked
    /// by pool_grant_is_priority_ordered. E5 supplies R7/R8; E7 supplies acceptance.
    fn pool_grant_step(mut w: World, i: usize, mut out: Outbox, mut report: Report)
        -> (result: (World, Outbox, Report, bool))
        requires checking::accepted(w), scheduling_safe(w),
            w.gates@.len() <= u64::MAX, w.nodes@.len() <= u64::MAX,
            i < w.nodes@.len(), i <= u64::MAX as int,
            advance_pre(w.nodes@[i as int], w.next_ticket, out.len),
            match current_item(w.nodes@[i as int]) { Some(Item::Start(_)) => true, _ => false },
            !(w.nodes@[i as int].phase is Closed), !w.nodes@[i as int].waiting_incident,
            w.nodes@[i as int].effect.is_none(), !w.nodes@[i as int].paused,
            w.nodes@[i as int].ticket.is_some(), !w.nodes@[i as int].pool,
        ensures checking::accepted(result.0),
            // R7: a new pool grant needs capacity and the first place in line.
            !w.nodes@[i as int].pool && result.0.nodes@[i as int].pool
                ==> pool_users(w.nodes@, w.nodes@.len()) < w.pool_capacity
                    && oldest_waiter(w, i as int),
            // R8: an eligible first waiter with room receives the pool place.
            (match current_item(w.nodes@[i as int]) { Some(Item::Start(_)) => true, _ => false })
                && !(w.nodes@[i as int].phase is Closed) && !w.nodes@[i as int].waiting_incident
                && w.nodes@[i as int].effect.is_none() && !w.nodes@[i as int].paused
                && w.nodes@[i as int].ticket.is_some()
                && pool_users(w.nodes@, w.nodes@.len()) < w.pool_capacity && oldest_waiter(w, i as int)
                ==> result.3 && result.0.nodes@[i as int].pool
                    && result.0.nodes@[i as int].ticket.is_none(),
            result.0.nodes@[i as int].pool ==> result.0.nodes@[i as int].ticket.is_none()
                && forall|j: int| 0 <= j < w.nodes@.len()
                    && !(w.nodes@[j].phase is Ready) && w.nodes@[j].ticket.is_some()
                    ==> !(priority_number(w.nodes@[j].priority) < priority_number(w.nodes@[i as int].priority)
                        || (w.nodes@[j].priority == w.nodes@[i as int].priority
                            && w.nodes@[j].ticket.unwrap() < w.nodes@[i as int].ticket.unwrap())),
            result.3 ==> 4 * node_distance(result.0.nodes@[i as int]) + pending_rank(result.0.nodes@[i as int])
                < 4 * node_distance(w.nodes@[i as int]) + pending_rank(w.nodes@[i as int]),
            transition_safety::transition_safety(w, result.0,
                transition_safety::Transition::Advance { node: i, moved: result.3 },
                transition_safety::emitted(out, result.1)),
    {
        let ghost before = w;
        let changed = scheduler::advance(&mut w, i, &mut out, &mut report);
        proof {
            advance_family_frames::step_premises_lift(before, w, i as int);
            internal_safety::step_preserves_accepted_full(before, w, i as int);
            assert(!(before.nodes@[i as int].phase is Ready));
            pool_line_order(before, i as int);
            advance_projection::settle_rank(before.nodes@[i as int], w.nodes@[i as int],
                before.next_ticket, w.next_ticket, changed);
        }
        (w, out, report, changed)
    }
    }
}

mod command_projection {
    use super::*;

    verus! {
    /// Resume changes paused/last_cmd, neither of which is a dependency endpoint.
    /// Project the owner's opaque Resume contract outside the consumer's group.
    pub(super) proof fn resume_dependencies(before: World, after: World, i: int, id: u64)
        requires checking::accepted(before), 0 <= i < before.nodes@.len(),
            after.dependencies@ == before.dependencies@,
            after.nodes@.len() == before.nodes@.len(),
            forall|j: int| 0 <= j < before.nodes@.len() && j != i
                ==> after.nodes@[j] == before.nodes@[j],
            progress_gate_apply::resume_post(before.nodes@[i], after.nodes@[i], id, Command::Resume),
        ensures dependencies::rules_ok(after),
    {
        checking::accepted_parts(before);
        reveal(progress_gate_apply::resume_post);
        reveal(progress_gate_apply::resumed);
        assert forall|j: int| 0 <= j < before.nodes@.len()
            implies dependencies::same_endpoint(#[trigger] before.nodes@[j], after.nodes@[j]) by {
            if j != i { assert(after.nodes@[j] == before.nodes@[j]); }
        }
        dependencies::frame_preserves(before, after);
    }
    }
}

mod resume_assembly {
    use super::*;

    verus! {
    /// R28 for the real Resume command entry, including repeated command IDs.
    /// Apply dispatch forwarding and invalid-node stutter remain separate rows.
    fn resume_command_step(mut w: World, event: Event, mut out: Outbox)
        -> (result: (World, Outbox))
        requires checking::accepted(w), out.len < OUTBOX, w.next_question < u64::MAX,
            match event {
                Event::Command { node, command: Command::Resume, .. } =>
                    node.0 < w.nodes@.len() && node.0 <= usize::MAX
                        && node_headroom(w.nodes@[node.0 as int])
                        && arithmetic_consistent(w.nodes@[node.0 as int]),
                _ => false,
            },
        ensures (checking::accepted(w) && (event is Command)) ==> checking::accepted(result.0),
    {
        let ghost before = w;
        match event {
            Event::Command { node, id, command: Command::Resume } => {
                let i = node.0 as usize;
                on_command(&mut w, i, id, Command::Resume, &mut out);
                proof {
                    checking::accepted_node(before, i as int);
                    command_projection::resume_dependencies(before, w, i as int, id);
                    internal_safety::apply_step_preserves_accepted(before, w, i as int);
                }
            },
            _ => {},
        }
        (w, out)
    }
    }
}

mod pause_projection {
    use super::*;

    verus! {
    pub(super) proof fn pause_dependencies(before: World, after: World, i: int)
        requires checking::accepted(before), 0 <= i < before.nodes@.len(),
            after.dependencies@ == before.dependencies@,
            after.nodes@.len() == before.nodes@.len(),
            forall|j: int| 0 <= j < before.nodes@.len() && j != i
                ==> after.nodes@[j] == before.nodes@[j],
            dependencies::same_endpoint(before.nodes@[i], after.nodes@[i]),
        ensures dependencies::rules_ok(after),
    {
        checking::accepted_parts(before);
        assert forall|j: int| 0 <= j < before.nodes@.len()
            implies dependencies::same_endpoint(#[trigger] before.nodes@[j], after.nodes@[j]) by {
            if j != i { assert(after.nodes@[j] == before.nodes@[j]); }
        }
        dependencies::frame_preserves(before, after);
    }
    }
}

mod pause_assembly {
    use super::*;

    verus! {
    fn pause_command_step(mut w: World, event: Event, mut out: Outbox)
        -> (result: (World, Outbox))
        requires checking::accepted(w), out.len < OUTBOX, w.next_question < u64::MAX,
            match event {
                Event::Command { node, command: Command::Pause, .. } =>
                    node.0 < w.nodes@.len() && node.0 <= usize::MAX
                        && node_headroom(w.nodes@[node.0 as int])
                        && arithmetic_consistent(w.nodes@[node.0 as int]),
                _ => false,
            },
        ensures (checking::accepted(w) && (event is Command)) ==> checking::accepted(result.0),
    {
        let ghost before = w;
        match event {
            Event::Command { node, id, command: Command::Pause } => {
                let i = node.0 as usize;
                on_command(&mut w, i, id, Command::Pause, &mut out);
                proof {
                    checking::accepted_node(before, i as int);
                    pause_projection::pause_dependencies(before, w, i as int);
                    internal_safety::apply_step_preserves_accepted(before, w, i as int);
                }
            },
            _ => {},
        }
        (w, out)
    }
    }
}

mod stop_assembly {
    use super::*;

    verus! {
    fn stop_command_step(mut w: World, event: Event, mut out: Outbox)
        -> (result: (World, Outbox))
        requires checking::accepted(w), out.len < OUTBOX, w.next_question < u64::MAX,
            w.nodes@.len() <= u64::MAX,
            match event {
                Event::Command { node, command: Command::Stop, .. } =>
                    node.0 < w.nodes@.len() && node.0 <= usize::MAX
                        && node_headroom(w.nodes@[node.0 as int])
                        && arithmetic_consistent(w.nodes@[node.0 as int]),
                _ => false,
            },
        ensures (checking::accepted(w) && (event is Command)) ==> checking::accepted(result.0),
    {
        let ghost before = w;
        match event {
            Event::Command { node, id, command: Command::Stop } => {
                let i = node.0 as usize;
                proof {
                    checking::accepted_parts(before);
                    if !dependencies::prerequisites_delivered(before, NodeId(i as u64)) {
                        dependency_owner_exports::waiting_owner_ok(before, i as int);
                    }
                }
                on_command(&mut w, i, id, Command::Stop, &mut out);
                proof {
                    checking::accepted_node(before, i as int);
                    handler_safety::step_withheld(before.nodes@[i as int], w.nodes@[i as int]);
                    internal_safety::apply_step_preserves_accepted_full(before, w, i as int);
                }
            },
            _ => {},
        }
        (w, out)
    }
    }
}

mod change_assembly {
    use super::*;

    verus! {
    fn change_request_command_step(mut w: World, event: Event, mut out: Outbox)
        -> (result: (World, Outbox))
        requires checking::accepted(w), out.len < OUTBOX, w.next_question < u64::MAX,
            w.nodes@.len() <= u64::MAX,
            match event {
                Event::Command { node, command: Command::ChangeRequest, .. } =>
                    node.0 < w.nodes@.len() && node.0 <= usize::MAX
                        && node_headroom(w.nodes@[node.0 as int])
                        && arithmetic_consistent(w.nodes@[node.0 as int]),
                _ => false,
            },
        ensures (checking::accepted(w) && (event is Command)) ==> checking::accepted(result.0),
    {
        let ghost before = w;
        match event {
            Event::Command { node, id, command: Command::ChangeRequest } => {
                let i = node.0 as usize;
                proof {
                    checking::accepted_parts(before);
                    if !dependencies::prerequisites_delivered(before, NodeId(i as u64)) {
                        dependency_owner_exports::waiting_owner_ok(before, i as int);
                    }
                }
                on_command(&mut w, i, id, Command::ChangeRequest, &mut out);
                proof {
                    checking::accepted_node(before, i as int);
                    handler_safety::step_withheld(before.nodes@[i as int], w.nodes@[i as int]);
                    internal_safety::apply_step_preserves_accepted_full(before, w, i as int);
                }
            },
            _ => {},
        }
        (w, out)
    }
    }
}

mod gate_projection {
    use super::*;

    verus! {
    pub(super) proof fn older_waiters(before: World, i: int)
        requires 0 <= i < before.nodes@.len(), before.nodes@[i].ticket.is_some(),
            forall|older: int| 0 <= older < before.nodes@.len()
                && before.nodes@[older].phase is Ready
                && before.nodes@[older].repo == before.nodes@[i].repo
                && before.nodes@[older].ticket.is_some()
                && before.nodes@[older].ticket.unwrap() < before.nodes@[i].ticket.unwrap()
                ==> footprint::disjoint(before, before.nodes@[older].footprint, before.nodes@[i].footprint),
        ensures !overlapped_by_older(before, i, before.nodes@[i].ticket.unwrap()),
    {
    }
    }
}

mod gate_ticket_projection {
    use super::*;

    verus! {
    /// E5.8 is consumed verbatim; a Ready node had no slot before the step.
    pub(super) proof fn ticketed_grant(b: Node, n: Node)
        requires !b.holds,
            !b.holds && n.holds ==> b.phase is Ready && b.ticket.is_some(),
        ensures n.holds ==> b.ticket.is_some(),
    {
    }
    }
}

mod gate_assembly {
    use super::*;

    verus! {
    fn gate_grant_step(mut w: World, i: usize, mut out: Outbox, mut report: Report)
        -> (result: (World, Outbox, Report, bool))
        requires checking::accepted(w), scheduling_safe(w),
            w.gates@.len() <= u64::MAX, w.nodes@.len() <= u64::MAX,
            i < w.nodes@.len(), i <= u64::MAX as int,
            advance_pre(w.nodes@[i as int], w.next_ticket, out.len),
        ensures checking::accepted(result.0),
            // R29 includes older waiters blocked on another holder.
            (w.nodes@[i as int].phase is Ready && w.nodes@[i as int].ticket.is_some()
                && !w.nodes@[i as int].holds && result.0.nodes@[i as int].holds)
                ==> forall|older: int| 0 <= older < w.nodes@.len()
                    && w.nodes@[older].phase is Ready && w.nodes@[older].repo == w.nodes@[i as int].repo
                    && w.nodes@[older].ticket.is_some()
                    && w.nodes@[older].ticket.unwrap() < w.nodes@[i as int].ticket.unwrap()
                    ==> footprint::disjoint(w, w.nodes@[older].footprint, w.nodes@[i as int].footprint),
            advance_family_post::ready_unobstructed(w.nodes@[i as int])
                && slot_spec(w, i as int) && oldest_waiter(w, i as int)
                ==> result.3 && result.0.nodes@[i as int].holds && result.0.nodes@[i as int].ticket.is_none(),
            advance_family_post::ready_unobstructed(w.nodes@[i as int])
                ==> forall|older: int| 0 <= older < w.nodes@.len()
                    && w.nodes@[older].phase is Ready && w.nodes@[older].repo == w.nodes@[i as int].repo
                    && w.nodes@[older].ticket.is_some()
                    && w.nodes@[older].ticket.unwrap() < w.nodes@[i as int].ticket.unwrap()
                    && slot_spec(w, older)
                    && !overlapped_by_older(w, older, w.nodes@[older].ticket.unwrap())
                    ==> !result.3 && result.0.nodes@[i as int] == w.nodes@[i as int],
            result.0.nodes@[i as int].holds ==> !w.nodes@[i as int].paused && slot_spec(w, i as int)
                && result.0.nodes@[i as int].ticket.is_none()
                && result.0.nodes@[i as int].pool == w.nodes@[i as int].pool
                && result.0.nodes@[i as int].run == w.nodes@[i as int].run,
            // E5.8 removes the old-ticket side condition from grant fairness.
            result.0.nodes@[i as int].holds ==> w.nodes@[i as int].ticket.is_some()
                && !overlapped_by_older(w, i as int, w.nodes@[i as int].ticket.unwrap()),
            result.0.nodes@[i as int].holds
                ==> forall|older: int| 0 <= older < w.nodes@.len()
                    && w.nodes@[older].phase is Ready && w.nodes@[older].repo == w.nodes@[i as int].repo
                    && w.nodes@[older].ticket.is_some()
                    && w.nodes@[older].ticket.unwrap() < w.nodes@[i as int].ticket.unwrap()
                    ==> !(slot_spec(w, older)
                        && !overlapped_by_older(w, older, w.nodes@[older].ticket.unwrap())),
            !result.0.nodes@[i as int].holds && w.nodes@[i as int].ticket.is_some()
                && !w.nodes@[i as int].paused ==> result.0.nodes@[i as int].ticket == w.nodes@[i as int].ticket,
            result.3 ==> 4 * node_distance(result.0.nodes@[i as int]) + pending_rank(result.0.nodes@[i as int])
                < 4 * node_distance(w.nodes@[i as int]) + pending_rank(w.nodes@[i as int]),
            transition_safety::transition_safety(w, result.0,
                transition_safety::Transition::Advance { node: i, moved: result.3 },
                transition_safety::emitted(out, result.1)),
        {
            let ghost before = w;
            let changed = scheduler::advance(&mut w, i, &mut out, &mut report);
            proof {
                checking::accepted_node(before, i as int);
                gate_ticket_projection::ticketed_grant(before.nodes@[i as int], w.nodes@[i as int]);
                if w.nodes@[i as int].holds {
                    assert(advance_family_post::ready_unobstructed(before.nodes@[i as int]));
                }
                advance_family_frames::step_premises_lift(before, w, i as int);
                internal_safety::step_preserves_accepted_full(before, w, i as int);
                checking::accepted_node(w, i as int);
                if w.nodes@[i as int].holds && before.nodes@[i as int].ticket.is_some() {
                    gate_projection::older_waiters(before, i as int);
                }
                advance_projection::settle_rank(before.nodes@[i as int], w.nodes@[i as int],
                    before.next_ticket, w.next_ticket, changed);
            }
            (w, out, report, changed)
        }
    }
}

/// Projections of the owner's opaque advance contract live outside the
/// consumer module so its representation stays local to this proof.
mod advance_projection {
    use super::*;

    verus! {
    pub(super) proof fn settle_rank(before: Node, after: Node,
        ticket_before: u64, ticket_after: u64, changed: bool)
        requires advance_post(before, after, ticket_before, ticket_after, changed),
        ensures changed ==> 4 * node_distance(after) + pending_rank(after)
            < 4 * node_distance(before) + pending_rank(before),
    {
        reveal(advance_post);
        node_distance_definition(before);
        node_distance_definition(after);
    }
    }
}

verus! {
/// G20 gate_planning_w2/w3 over arbitrary accepted worlds. This verification
/// entry calls the real scheduler; production has no caller of this wrapper.
/// E5's node rows are lifted by the owner's frame lemma, then E7 proves the
/// checker invariant. No constructor equality is exported by this consumer.
fn gate_planning_step(mut w: World, i: usize, mut out: Outbox, mut report: Report)
    -> (result: (World, Outbox, Report, bool))
    requires checking::accepted(w), scheduling_safe(w),
        w.gates@.len() <= u64::MAX, w.nodes@.len() <= u64::MAX,
        i < w.nodes@.len(), i <= u64::MAX as int,
        advance_pre(w.nodes@[i as int], w.next_ticket, out.len),
        w.nodes@[i as int].phase == Phase::Unplanned,
        w.nodes@[i as int].pos == 0, w.nodes@[i as int].ticket.is_none(),
    ensures checking::accepted(result.0), // R6: checker acceptance after an internal step.
        !result.0.nodes@[i as int].holds, !result.0.nodes@[i as int].pool,
        result.3 == (!w.nodes@[i as int].paused && !w.nodes@[i as int].waiting_incident
            && w.nodes@[i as int].effect.is_none()
            && dependencies::delivered(w.dependencies@, NodeId(i as u64))
            && w.nodes@[i as int].repo.0 < w.gates@.len()
            && slot_waiters(w.nodes@, w.nodes@.len(), w.nodes@[i as int].repo)
                < w.gates@[w.nodes@[i as int].repo.0 as int].w),
        result.3 ==> result.0.nodes@[i as int].phase == (Phase::Planning { must_split: false }),
        result.3 ==> 4 * node_distance(result.0.nodes@[i as int]) + pending_rank(result.0.nodes@[i as int])
            < 4 * node_distance(w.nodes@[i as int]) + pending_rank(w.nodes@[i as int]),
        // E5.2 framing: keep the owner's opaque relation as the interface.
        transition_safety::transition_safety(w, result.0,
            transition_safety::Transition::Advance { node: i, moved: result.3 },
            transition_safety::emitted(out, result.1)),
{
    let ghost before = w;
    let changed = scheduler::advance(&mut w, i, &mut out, &mut report);
    proof {
        advance_family_frames::step_premises_lift(before, w, i as int);
        internal_safety::step_preserves_accepted_full(before, w, i as int);
        gate_planning_holds_nothing(before, i as int, w.nodes@[i as int],
            before.next_ticket, w.next_ticket, changed);
        advance_projection::settle_rank(before.nodes@[i as int], w.nodes@[i as int],
            before.next_ticket, w.next_ticket, changed);
    }
    (w, out, report, changed)
}
}
