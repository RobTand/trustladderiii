//! #805 gate/pool obstruction and selected fact premises.
use super::*;
verus! {
/// Q30/Q39 / #805: discharge this part of the real scheduler obstruction.
pub(super) proof fn no_outside_no_holders(w: World)
    requires world_checked(w), settled_world(w),
        forall|j: int| 0 <= j < w.nodes@.len() ==> !outside_available(w, j),
    ensures forall|j: int| 0 <= j < w.nodes@.len() ==> !w.nodes@[j].holds,
{
    no_outside_no_pool_line(w);
    reveal(node_checked);
    reveal(outside_available);
    reveal(movable);
    reveal(oldest_waiter);
    reveal(pool_precedes);
    reveal(pool_line);
    world_checked_definition(w);
    settled_world_definition(w);
    assert forall|j: int| 0 <= j < w.nodes@.len() implies !w.nodes@[j].holds by {
        let n = w.nodes@[j];
        assert(node_checked(w, n));
        assert(!outside_available(w, j));
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

/// Q30/Q39: the minimum Ready ticket is first in its repository slot line.
pub(super) proof fn first_ready_waiter(w: World)
    requires exists|j: int| 0 <= j < w.nodes@.len() && w.nodes@[j].phase is Ready,
        forall|j: int| 0 <= j < w.nodes@.len() && w.nodes@[j].phase is Ready ==> w.nodes@[j].ticket.is_some(),
    ensures exists|i: int| 0 <= i < w.nodes@.len() && w.nodes@[i].phase is Ready && oldest_waiter(w, i),
{
    reveal(oldest_waiter);
    finite_minimum(w.nodes@, w.nodes@.len(), |n: Node| n.phase is Ready,
        |n: Node| n.ticket.unwrap() as nat);
    let i = choose|i: int| 0 <= i < w.nodes@.len() && w.nodes@[i].phase is Ready
        && forall|j: int| 0 <= j < w.nodes@.len() && w.nodes@[j].phase is Ready
            ==> w.nodes@[i].ticket.unwrap() <= w.nodes@[j].ticket.unwrap();
    assert(oldest_waiter(w, i));
}

/// Q30/Q39 / #805: discharge this part of the real scheduler obstruction.
pub(super) proof fn no_outside_no_ready(w: World)
    requires world_checked(w), settled_world(w),
        forall|j: int| 0 <= j < w.nodes@.len() ==> !outside_available(w, j),
    ensures forall|j: int| 0 <= j < w.nodes@.len() ==> !(w.nodes@[j].phase is Ready),
{
    no_outside_no_holders(w);
    reveal(node_checked);
    reveal(outside_available);
    reveal(movable);
    reveal(oldest_waiter);
    reveal(pool_precedes);
    reveal(pool_line);
    world_checked_definition(w);
    settled_world_definition(w);
    assert forall|j: int| 0 <= j < w.nodes@.len() && w.nodes@[j].phase is Ready
        implies w.nodes@[j].ticket.is_some() by {
        assert(node_checked(w, w.nodes@[j]));
        assert(!outside_available(w, j));
        if w.nodes@[j].ticket.is_none() { assert(movable(w, j)); }
    }
    if exists|j: int| 0 <= j < w.nodes@.len() && w.nodes@[j].phase is Ready {
        first_ready_waiter(w);
        let i = choose|i: int| 0 <= i < w.nodes@.len() && w.nodes@[i].phase is Ready && oldest_waiter(w, i);
        assert(node_checked(w, w.nodes@[i]));
        assert(!outside_available(w, i));
        assert(movable(w, i));
        assert(false);
    }
}

/// Q30/Q39 / #805: discharge this part of the real scheduler obstruction.
pub(super) proof fn outside_branch_exists(w: World)
    requires world_checked(w), settled_world(w),
        exists|i: int| 0 <= i < w.nodes@.len() && !(w.nodes@[i].phase is Closed),
    ensures exists|i: int| 0 <= i < w.nodes@.len() && outside_available(w, i),
{
    reveal(node_checked);
    reveal(outside_available);
    reveal(movable);
    reveal(oldest_waiter);
    reveal(pool_precedes);
    reveal(pool_line);
    world_checked_definition(w);
    settled_world_definition(w);
    if !(exists|j: int| 0 <= j < w.nodes@.len() && outside_available(w, j)) {
        no_outside_no_pool(w);
        no_outside_no_pool_line(w);
        no_outside_no_ready(w);
        let i = choose|i: int| 0 <= i < w.nodes@.len() && !(w.nodes@[i].phase is Closed);
        let n = w.nodes@[i];
        assert(node_checked(w, n));
        assert(!outside_available(w, i));
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

/// Q30/Q39 / #805: discharge this part of the real scheduler obstruction.
pub(super) proof fn resume_decreases(w: World, n: Node)
    requires node_checked(w, n), n.paused, !(n.phase is Closed || n.phase is Stopping),
    ensures distance(Node { paused: false, ..n }) < distance(n),
{
    reveal(distance);
    reveal(node_checked);
    match n.phase {
        Phase::Ingesting | Phase::Auditing | Phase::Unplanned | Phase::Planning { .. }
            | Phase::Ready(_) | Phase::Making | Phase::Checking | Phase::Reviewing
            | Phase::Publishing | Phase::Queued | Phase::Rebasing | Phase::AwaitingRob
            | Phase::Stopping(_) | Phase::Closed(_) => {},
    }
}

/// Q30/Q39 / #805: discharge this part of the real scheduler obstruction.
pub(super) proof fn fact_witness_premises(w: World, i: int, fact: Fact)
    requires 0 <= i < w.nodes@.len(), i <= u64::MAX as int, fact_progress_input(w, i, fact),
    ensures node_checked(w, w.nodes@[i]),
        match fact {
            Fact::Dependency { target, status } => (status == DependencyStatus::Delivered && dependencies::delivery_input(w, i, target)),
            Fact::PrMerged { .. } => w.nodes@[i].phase == Phase::Queued
                && !w.nodes@[i].paused && !w.nodes@[i].waiting_incident
                && w.nodes@[i].effect.is_none() && w.nodes@[i].question.is_none(),
            _ => fact == Fact::IncidentCleared { by_rob: true }
                && w.nodes@[i].waiting_incident && w.nodes@[i].effect.is_none()
                && (!w.nodes@[i].paused || w.nodes@[i].phase is Stopping)
                && !(w.nodes@[i].phase is Closed),
        },
{
    reveal(fact_progress_input);
    reveal(chosen_for_node);
    reveal(node_checked);
    reveal(outside_available);
    reveal(movable);
    reveal(oldest_waiter);
    reveal(pool_precedes);
    reveal(pool_line);
    world_checked_definition(w);
    if let Fact::Dependency { target, status } = fact {
        dependencies::pending_ready(w, i, target);
    }
}
/// #805: explicitly instantiate the real witness selector's changed node.
pub(super) proof fn selected_event(w: World, i: int, event: Event)
    requires 0 <= i < w.nodes@.len(), chosen_for_node(w, i, event),
    ensures witness_selection(w, Some(event)),
{
    reveal(witness_selection);
    assert(exists|j: int| 0 <= j < w.nodes@.len() && chosen_for_node(w, j, event));
}
}
