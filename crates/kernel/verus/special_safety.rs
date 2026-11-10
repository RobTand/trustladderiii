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
    pub(super) fn pool_grant_step(mut w: World, i: usize, mut out: Outbox, mut report: Report)
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
    pub(super) fn resume_command_step(mut w: World, event: Event, mut out: Outbox)
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
    pub(super) fn pause_command_step(mut w: World, event: Event, mut out: Outbox)
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
    pub(super) fn stop_command_step(mut w: World, event: Event, mut out: Outbox)
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
                    command_frames_reveal::command_frame_rows_reveal(
                        before.nodes@[i as int], w.nodes@[i as int], id, Command::Stop);
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
    pub(super) fn change_request_command_step(mut w: World, event: Event, mut out: Outbox)
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
                    command_frames_reveal::command_frame_rows_reveal(
                        before.nodes@[i as int], w.nodes@[i as int], id, Command::ChangeRequest);
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
    pub(super) fn gate_grant_step(mut w: World, i: usize, mut out: Outbox, mut report: Report)
        -> (result: (World, Outbox, Report, bool))
        requires checking::accepted(w), scheduling_safe(w),
            w.gates@.len() <= u64::MAX, w.nodes@.len() <= u64::MAX,
            i < w.nodes@.len(), i <= u64::MAX as int,
            advance_pre(w.nodes@[i as int], w.next_ticket, out.len),
            w.nodes@[i as int].phase is Ready,
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

mod first_line_assembly {
    use super::*;

    verus! {
    /// E5.9 supplies sufficiency; the query's existing rows supply necessity.
    pub(super) fn full_first_in_line(w: &World, i: usize) -> (result: bool)
        requires i < w.nodes@.len(), scheduling_safe(*w),
        ensures w.nodes@[i as int].phase is Ready ==> result == (
            w.nodes@[i as int].ticket.is_some()
            && (forall|older: int| 0 <= older < w.nodes@.len()
                && w.nodes@[older].phase is Ready
                && w.nodes@[older].repo == w.nodes@[i as int].repo
                && w.nodes@[older].ticket.is_some()
                && w.nodes@[older].ticket.unwrap() < w.nodes@[i as int].ticket.unwrap()
                ==> footprint::disjoint(*w, w.nodes@[older].footprint, w.nodes@[i as int].footprint))
            && (forall|older: int| 0 <= older < w.nodes@.len()
                && w.nodes@[older].phase is Ready
                && w.nodes@[older].repo == w.nodes@[i as int].repo
                && w.nodes@[older].ticket.is_some()
                && w.nodes@[older].ticket.unwrap() < w.nodes@[i as int].ticket.unwrap()
                ==> !(slot_spec(*w, older)
                    && !overlapped_by_older(*w, older, w.nodes@[older].ticket.unwrap())))),
            !(w.nodes@[i as int].phase is Ready) ==> result == oldest_waiter(*w, i as int),
    {
        scheduler::first_in_line(w, i)
    }
    }
}

mod gate_first_line_assembly {
    use super::*;

    verus! {
    /// Observe the executable query before the real advance step.
    fn gate_first_in_line_step(mut w: World, i: usize, out: Outbox, report: Report)
        -> (result: (World, Outbox, Report, bool, bool))
        requires checking::accepted(w), scheduling_safe(w),
            w.gates@.len() <= u64::MAX, w.nodes@.len() <= u64::MAX,
            i < w.nodes@.len(), i <= u64::MAX as int,
            advance_pre(w.nodes@[i as int], w.next_ticket, out.len),
            w.nodes@[i as int].phase is Ready,
        ensures checking::accepted(result.0),
            // H59/H60: a slot grant requires the actual pre-step first_in_line result.
            result.0.nodes@[i as int].holds ==> result.4,
    {
        let first = first_line_assembly::full_first_in_line(&w, i);
        let (after, out, report, changed) = gate_assembly::gate_grant_step(w, i, out, report);
        (after, out, report, changed, first)
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


mod resume_apply_projection {
    use super::*;

    verus! {
    pub(super) proof fn endpoint(before: World, after: World, node: NodeId, id: u64)
        requires node.0 < before.nodes@.len(),
            progress_gate_apply::resume_apply_post(before, after,
                Event::Command { node, id, command: Command::Resume }),
        ensures dependencies::same_endpoint(before.nodes@[node.0 as int], after.nodes@[node.0 as int]),
    {
        reveal(progress_gate_apply::resume_apply_post);
        reveal(progress_gate_apply::resumed);
    }
    }
}

mod aggregate_command_rows {
    use super::*;

    verus! {
    /// The callee's per-field posts are premises, without a model-link premise.
    pub(super) proof fn acceptance(before: World, after: World, node: NodeId, command: Command)
        requires checking::accepted(before), node.0 < before.nodes@.len(),
            matches!(command, Command::Stop | Command::Pause | Command::Resume | Command::ChangeRequest),
            after.nodes@.len() == before.nodes@.len(),
            after.dependencies@ == before.dependencies@,
            before.next_question <= after.next_question,
            before.nodes@.len() <= u64::MAX,
            retirement::node_rules(before, before.nodes@[node.0 as int])
                ==> retirement::node_rules(after, after.nodes@[node.0 as int]),
            after.nodes@[node.0 as int].holds ==> before.nodes@[node.0 as int].holds,
            after.nodes@[node.0 as int].pool ==> before.nodes@[node.0 as int].pool,
            after.nodes@[node.0 as int].ticket == before.nodes@[node.0 as int].ticket
                || after.nodes@[node.0 as int].ticket.is_none(),
            after.nodes@[node.0 as int].repo == before.nodes@[node.0 as int].repo,
            after.nodes@[node.0 as int].footprint == before.nodes@[node.0 as int].footprint,
            after.nodes@ == before.nodes@.update(node.0 as int, after.nodes@[node.0 as int]),
            handler_safety::command_step(before.nodes@[node.0 as int], after.nodes@[node.0 as int]),
            ((command is Stop || command is ChangeRequest)
                && dependencies::owner_waiting_ok(before.nodes@[node.0 as int]))
                ==> dependencies::owner_waiting_ok(after.nodes@[node.0 as int]),
            (command is Pause || command is Resume)
                ==> dependencies::same_endpoint(before.nodes@[node.0 as int], after.nodes@[node.0 as int]),
            after.gates@ == before.gates@,
            after.pool_capacity == before.pool_capacity,
            after.next_ticket == before.next_ticket,
            after.file_sets@.len() >= before.file_sets@.len(),
            forall|j: int| 0 <= j < before.file_sets@.len()
                ==> after.file_sets@[j] == before.file_sets@[j],
            forall|j: int| 0 <= j < before.nodes@.len() ==>
                (before.nodes@[j].phase is Closed ==> after.nodes@[j].phase is Closed)
                    && after.nodes@[j].repo == before.nodes@[j].repo
                    && after.nodes@[j].last_cmd >= before.nodes@[j].last_cmd,
        ensures checking::accepted(after),
    {
        let i = node.0 as int;
        checking::accepted_node(before, i);
        if command is Pause || command is Resume {
            pause_projection::pause_dependencies(before, after, i);
            internal_safety::apply_step_preserves_accepted(before, after, i);
        } else {
            checking::accepted_parts(before);
            if !dependencies::prerequisites_delivered(before, node) {
                dependency_owner_exports::waiting_owner_ok(before, i);
            }
            handler_safety::step_withheld(before.nodes@[i], after.nodes@[i]);
            internal_safety::apply_step_preserves_accepted_full(before, after, i);
        }
    }
    }
}

mod aggregate_command_assembly {
    use super::*;

    verus! {
    /// R28 for all targets and fresh/stale IDs of the four qualified commands.
    pub(super) fn apply_non_answer_step(mut w: World, event: Event, mut out: Outbox, mut report: Report)
        -> (result: (World, Outbox, Report))
        requires checking::accepted(w),
            world_headroom(w), footprint::ordered_table(w), dependencies::edges_indexable(w),
            out.len + 4 <= OUTBOX,
            forall|j: int| 0 <= j < w.nodes@.len() ==>
                arithmetic_consistent(w.nodes@[j]) && settle_shape(w.nodes@[j])
                    && stopping_effect_fits(w.nodes@[j])
                    && (w.nodes@[j].phase is Closed ==> w.nodes@[j].effect.is_none())
                    && footprint::valid_footprint(w, w.nodes@[j].footprint),
            match event {
                Event::Command { command, .. } => matches!(command,
                    Command::Stop | Command::Pause | Command::Resume | Command::ChangeRequest),
                _ => false,
            },
        ensures (checking::accepted(w) && (event is Command)) ==> checking::accepted(result.0),
    {
        let ghost before = w;
        events::apply(&mut w, event, &mut out, &mut report);
        proof {
            match event {
                Event::Command { node, id, command } => {
                    command_frames_reveal::command_rows_project(before, w, node, id, command);
                    if node.0 < before.nodes@.len() {
                        checking::accepted_node(before, node.0 as int);
                        command_frames_reveal::command_frame_rows_reveal(
                            before.nodes@[node.0 as int], w.nodes@[node.0 as int], id, command);
                        if command is Resume {
                            resume_apply_projection::endpoint(before, w, node, id);
                        }
                        aggregate_command_rows::acceptance(before, w, node, command);
                    } else {
                        checking::accepted_frame(before, w);
                    }
                },
                _ => {},
            }
        }
        (w, out, report)
    }
    }
}

mod invalid_command_assembly {
    use super::*;

    verus! {
    /// Includes Answer at an absent target; no valid-node Answer premise is used.
    pub(super) fn apply_invalid_target(mut w: World, event: Event, mut out: Outbox, mut report: Report)
        -> (result: (World, Outbox, Report))
        requires checking::accepted(w),
            world_headroom(w), footprint::ordered_table(w), dependencies::edges_indexable(w),
            out.len + 4 <= OUTBOX,
            forall|j: int| 0 <= j < w.nodes@.len() ==>
                arithmetic_consistent(w.nodes@[j]) && settle_shape(w.nodes@[j])
                    && stopping_effect_fits(w.nodes@[j])
                    && (w.nodes@[j].phase is Closed ==> w.nodes@[j].effect.is_none())
                    && footprint::valid_footprint(w, w.nodes@[j].footprint),
            match event {
                Event::Command { node, .. } => node.0 >= w.nodes@.len(),
                _ => false,
            },
        ensures (checking::accepted(w) && (event is Command)) ==> checking::accepted(result.0),
    {
        let ghost before = w;
        events::apply(&mut w, event, &mut out, &mut report);
        proof {
            match event {
                Event::Command { node, id, command } => {
                    command_frames_reveal::command_rows_project(before, w, node, id, command);
                    checking::accepted_frame(before, w);
                },
                _ => {},
            }
        }
        (w, out, report)
    }
    }
}


mod answer_frontier {
    use super::*;

    verus! {
    /// The proven Answer inputs to C1, without the absent final waiting-owner row.
    #[verifier::opaque]
    pub(super) open spec fn rows(before: World, after: World, node: NodeId) -> bool {
        checking::accepted(before)
            && node.0 < before.nodes@.len()
            && retirement::node_rules(before, before.nodes@[node.0 as int])
            && internal_safety::apply_n1_rules(after, node.0 as int)
            && internal_safety::apply_n2_no_grants(before.nodes@[node.0 as int], after.nodes@[node.0 as int])
            && internal_safety::apply_n3_footprint(after, before.nodes@[node.0 as int], node.0 as int)
            && internal_safety::apply_n5_frame(before, after, node.0 as int)
            && progress_gate_apply::accepted(after)
            && after.dependencies@ == before.dependencies@
            && before.nodes@.len() <= u64::MAX
            && (before.nodes@[node.0 as int].phase is Closed ==> after.nodes@[node.0 as int].phase is Closed)
            && (after.nodes@[node.0 as int].credit.withheld ==> before.nodes@[node.0 as int].credit.withheld
                || !dependencies::prerequisites_delivered(before, node))
            && (!dependencies::prerequisites_delivered(before, node)
                ==> dependencies::owner_waiting_ok(before.nodes@[node.0 as int]))
    }

    /// Rule 7: E1.5 and apply's per-field posts are taken verbatim.
    pub(super) proof fn rows_intro(before: World, after: World, node: NodeId, id: u64,
        question: u64, answer: Answer)
        requires checking::accepted(before), node.0 < before.nodes@.len(),
            after.nodes@.len() == before.nodes@.len(),
            after.dependencies@ == before.dependencies@,
            before.next_question <= after.next_question,
            before.nodes@.len() <= u64::MAX,
            retirement::node_rules(before, before.nodes@[node.0 as int])
                ==> retirement::node_rules(after, after.nodes@[node.0 as int]),
            after.nodes@[node.0 as int].holds ==> before.nodes@[node.0 as int].holds,
            after.nodes@[node.0 as int].pool ==> before.nodes@[node.0 as int].pool,
            after.nodes@[node.0 as int].ticket == before.nodes@[node.0 as int].ticket
                || after.nodes@[node.0 as int].ticket.is_none(),
            after.nodes@[node.0 as int].repo == before.nodes@[node.0 as int].repo,
            after.nodes@[node.0 as int].footprint == before.nodes@[node.0 as int].footprint,
            after.nodes@ == before.nodes@.update(node.0 as int, after.nodes@[node.0 as int]),
            handler_safety::answer_pair(before.nodes@[node.0 as int], after.nodes@[node.0 as int],
                id > before.nodes@[node.0 as int].last_cmd, question, answer, after.dependencies@, node),
            after.gates@ == before.gates@,
            after.pool_capacity == before.pool_capacity,
            after.next_ticket == before.next_ticket,
            after.file_sets@.len() >= before.file_sets@.len(),
            forall|j: int| 0 <= j < before.file_sets@.len()
                ==> after.file_sets@[j] == before.file_sets@[j],
            forall|j: int| 0 <= j < before.nodes@.len() ==>
                (before.nodes@[j].phase is Closed ==> after.nodes@[j].phase is Closed)
                    && after.nodes@[j].repo == before.nodes@[j].repo
                    && after.nodes@[j].last_cmd >= before.nodes@[j].last_cmd,
        ensures rows(before, after, node),
    {
        let i = node.0 as int;
        checking::accepted_node(before, i);
        checking::accepted_parts(before);
        if !dependencies::prerequisites_delivered(before, node) {
            dependency_owner_exports::waiting_owner_ok(before, i);
        }
        handler_safety::answer_pair_withheld(before.nodes@[i], after.nodes@[i],
            id > before.nodes@[i].last_cmd, question, answer, after.dependencies@, node);
        dependencies::prerequisite_definition(before, node);
        internal_safety::apply_preserves_gate_rules(before, after, i);
        reveal(rows);
    }
    }
}

mod answer_frontier_reveal {
    use super::*;

    verus! {
    pub(super) proof fn project(before: World, after: World, node: NodeId)
        requires answer_frontier::rows(before, after, node),
        ensures
            checking::accepted(before),
            node.0 < before.nodes@.len(),
            retirement::node_rules(before, before.nodes@[node.0 as int]),
            internal_safety::apply_n1_rules(after, node.0 as int),
            internal_safety::apply_n2_no_grants(before.nodes@[node.0 as int], after.nodes@[node.0 as int]),
            internal_safety::apply_n3_footprint(after, before.nodes@[node.0 as int], node.0 as int),
            internal_safety::apply_n5_frame(before, after, node.0 as int),
            progress_gate_apply::accepted(after),
            after.dependencies@ == before.dependencies@,
            before.nodes@.len() <= u64::MAX,
            (before.nodes@[node.0 as int].phase is Closed ==> after.nodes@[node.0 as int].phase is Closed),
            (after.nodes@[node.0 as int].credit.withheld ==> before.nodes@[node.0 as int].credit.withheld
                || !dependencies::prerequisites_delivered(before, node)),
            (!dependencies::prerequisites_delivered(before, node)
                ==> dependencies::owner_waiting_ok(before.nodes@[node.0 as int])),
    {
        reveal(answer_frontier::rows);
    }
    }
}

mod answer_apply_assembly {
    use super::*;

    verus! {
    /// H36-H43/S2 preparation over every Answer and fresh/stale command id.
    /// This prefix is not the complete R28 theorem while N4c is unavailable.
    fn apply_answer_prefix(mut w: World, event: Event, mut out: Outbox, mut report: Report)
        -> (result: (World, Outbox, Report))
        requires checking::accepted(w),
            world_headroom(w), footprint::ordered_table(w), dependencies::edges_indexable(w),
            out.len + 4 <= OUTBOX,
            forall|j: int| 0 <= j < w.nodes@.len() ==>
                arithmetic_consistent(w.nodes@[j]) && settle_shape(w.nodes@[j])
                    && stopping_effect_fits(w.nodes@[j])
                    && (w.nodes@[j].phase is Closed ==> w.nodes@[j].effect.is_none())
                    && footprint::valid_footprint(w, w.nodes@[j].footprint),
            match event {
                Event::Command { node, command: Command::Answer { .. }, .. } => node.0 < w.nodes@.len(),
                _ => false,
            },
        ensures match event {
            Event::Command { node, .. } => answer_frontier::rows(w, result.0, node),
            _ => true,
        },
    {
        let ghost before = w;
        events::apply(&mut w, event, &mut out, &mut report);
        proof {
            match event {
                Event::Command { node, id, command: Command::Answer { question, answer } } => {
                    command_frames_reveal::command_rows_project(before, w, node, id,
                        Command::Answer { question, answer });
                    answer_frontier::rows_intro(before, w, node, id, question, answer);
                    answer_frontier_reveal::project(before, w, node);
                    // N4c boundary: the next C1-full call needs prerequisites_delivered(before, node)
                    // || owner_waiting_ok(w.nodes@[node.0 as int]); no producer export supplies it yet.
                },
                _ => {},
            }
        }
        (w, out, report)
    }
    }
}

mod assembly_witness_world {
    use super::*;

    verus! {
    pub(super) fn world(kind: u8) -> (w: World)
        requires kind < 3,
        ensures checking::accepted(w), scheduling_safe(w), world_headroom(w),
            footprint::ordered_table(w), dependencies::edges_indexable(w),
            w.nodes@.len() == 1, w.gates@.len() == 1,
            w.next_question == 0, w.next_ticket == 8,
            node_headroom(w.nodes@[0]), arithmetic_consistent(w.nodes@[0]),
            settle_shape(w.nodes@[0]), stopping_effect_fits(w.nodes@[0]),
            footprint::valid_footprint(w, w.nodes@[0].footprint),
            advance_pre(w.nodes@[0], w.next_ticket, 0),
            !w.nodes@[0].paused, !w.nodes@[0].waiting_incident,
            w.nodes@[0].effect.is_none(), !w.nodes@[0].pool, !w.nodes@[0].holds,
            w.nodes@[0].pos == 0,
            w.nodes@[0].phase == if kind == 0 { Phase::Unplanned }
                else if kind == 1 { Phase::Ready(Resume::Making) } else { Phase::Ingesting },
            w.nodes@[0].ticket == if kind == 0 { None } else { Some(0) },
    {
        let mut n = Node::new(RepoId(0));
        n.credit.ready = true;
        n.credit.start = 4;
        n.attempt = 1;
        n.footprint = Footprint::inline(1);
        n.phase = if kind == 0 { Phase::Unplanned }
            else if kind == 1 { Phase::Ready(Resume::Making) } else { Phase::Ingesting };
        n.ticket = if kind == 0 { None } else { Some(0) };
        let mut nodes = Vec::new();
        nodes.push(n);
        let mut gates = Vec::new();
        gates.push(Gate { w: 2 });
        let w = World { nodes, gates, pool_capacity: 4, next_ticket: 8, next_question: 0,
            file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
        proof {
            reveal(initial_node);
            reveal(retirement::node_rules);
            reveal(footprint::ordered_table);
            footprint::inline_is_valid(w, 1);
            assert(retirement::node_rules(w, w.nodes@[0]));
            dependencies::rules_definition(w);
            dependencies::acyclic_definition(w);
            dependencies::indexable_definition(w);
            reveal(dependencies::active_edge);
            checking::accepted_definition(w);
            reveal_with_fuel(potential_prefix, 2);
            reveal_with_fuel(open_prefix, 2);
            reveal(world_headroom);
            assert forall|i: int| 0 <= i < w.nodes@.len()
                implies retirement::node_rules(w, w.nodes@[i]) by { assert(i == 0); }
            assert(checking::accepted(w));
        }
        w
    }
    }
}

mod assembly_entry_witnesses {
    use super::*;

    verus! {
    fn commands(case: u8)
        requires case < 4,
    {
        let w = assembly_witness_world::world(0);
        let out = Outbox::new();
        if case == 0 {
            let event = Event::Command { node: NodeId(0), id: 1, command: Command::Stop };
            let _ = stop_assembly::stop_command_step(w, event, out);
        } else if case == 1 {
            let event = Event::Command { node: NodeId(0), id: 1, command: Command::Pause };
            let _ = pause_assembly::pause_command_step(w, event, out);
        } else if case == 2 {
            let event = Event::Command { node: NodeId(0), id: 1, command: Command::Resume };
            let _ = resume_assembly::resume_command_step(w, event, out);
        } else {
            let event = Event::Command { node: NodeId(0), id: 1, command: Command::ChangeRequest };
            let _ = change_assembly::change_request_command_step(w, event, out);
        }
    }

    fn pool() {
        let w = assembly_witness_world::world(2);
        let _ = pool_assembly::pool_grant_step(w, 0, Outbox::new(), Report::new());
    }

    fn gate() {
        let w = assembly_witness_world::world(1);
        let _ = gate_assembly::gate_grant_step(w, 0, Outbox::new(), Report::new());
    }

    fn planning() {
        let w = assembly_witness_world::world(0);
        let _ = gate_planning_step(w, 0, Outbox::new(), Report::new());
    }

    fn aggregate() {
        let w = assembly_witness_world::world(0);
        let event = Event::Command { node: NodeId(0), id: 1, command: Command::Resume };
        let _ = aggregate_command_assembly::apply_non_answer_step(w, event, Outbox::new(), Report::new());
    }

    fn invalid() {
        let w = assembly_witness_world::world(0);
        let event = Event::Command { node: NodeId(1), id: 1, command: Command::Answer {
            question: 0, answer: Answer::Stop } };
        let _ = invalid_command_assembly::apply_invalid_target(w, event, Outbox::new(), Report::new());
    }
    }
}
