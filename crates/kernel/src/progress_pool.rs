//! Q39 / #805 pool obstruction, split from the global selection argument.
use super::*;
verus! {
/// Q30/Q39 / #805: discharge this part of the real scheduler obstruction.
pub(super) proof fn potential_zero(nodes: Seq<Node>, end: nat, potential: spec_fn(Node) -> nat)
    requires end <= nodes.len(), forall|j: int| 0 <= j < end ==> potential(nodes[j]) == 0,
    ensures potential_prefix(nodes, end, potential) == 0,
    decreases end,
{
    if end > 0 { potential_zero(nodes, (end - 1) as nat, potential); }
}

/// Q30/Q39 / #805: discharge this part of the real scheduler obstruction.
pub(super) proof fn no_outside_no_pool(w: World)
    requires world_checked(w), settled_world(w),
        forall|j: int| 0 <= j < w.nodes@.len() ==> !outside_available(w, j),
    ensures forall|j: int| 0 <= j < w.nodes@.len() ==> !w.nodes@[j].pool,
        pool_users(w.nodes@, w.nodes@.len()) == 0,
{
    reveal(node_checked);
    reveal(outside_available);
    reveal(movable);
    reveal(oldest_waiter);
    reveal(pool_precedes);
    reveal(pool_line);
    world_checked_definition(w);
    settled_world_definition(w);
    assert forall|j: int| 0 <= j < w.nodes@.len() implies !w.nodes@[j].pool by {
        let n = w.nodes@[j];
        assert(node_checked(w, n));
        assert(!outside_available(w, j));
        if n.pool {
            assert(n.run.is_some());
            assert(!(n.phase is Closed));
            assert(current_item(n) != Some(Item::Await));
            assert(n.phase is Stopping && n.pos == 0);
            assert(movable(w, j));
        }
    }
    potential_zero(w.nodes@, w.nodes@.len(), |n: Node| if n.pool { 1nat } else { 0nat });
}

/// Q30/Q39 / #805: discharge this part of the real scheduler obstruction.
pub(super) proof fn pool_order_strict(a: Node, b: Node)
    requires a.ticket.is_some(), b.ticket.is_some(), pool_precedes(a, b),
    ensures pool_order(a) < pool_order(b),
{
    reveal(pool_order);
    reveal(pool_precedes);
    reveal(priority_number);
    match a.priority { Priority::P0 => {}, Priority::P1 => {}, Priority::P2 => {},
        Priority::P3 => {}, Priority::Unranked => {} }
    match b.priority { Priority::P0 => {}, Priority::P1 => {}, Priority::P2 => {},
        Priority::P3 => {}, Priority::Unranked => {} }
}

/// Q39: a nonempty finite pool line has a first priority/ticket key.
pub(super) proof fn first_pool_waiter(w: World)
    requires exists|j: int| 0 <= j < w.nodes@.len() && pool_line(w.nodes@[j]),
    ensures exists|i: int| 0 <= i < w.nodes@.len() && pool_line(w.nodes@[i]) && oldest_waiter(w, i),
{
    reveal(pool_line);
    reveal(oldest_waiter);
    reveal(pool_precedes);
    finite_minimum(w.nodes@, w.nodes@.len(), |n: Node| pool_line(n), |n: Node| pool_order(n));
    let i = choose|i: int| 0 <= i < w.nodes@.len() && pool_line(w.nodes@[i])
        && forall|j: int| 0 <= j < w.nodes@.len() && pool_line(w.nodes@[j])
            ==> pool_order(w.nodes@[i]) <= pool_order(w.nodes@[j]);
    assert forall|j: int| 0 <= j < w.nodes@.len() implies !pool_precedes(w.nodes@[j], w.nodes@[i]) by {
        if pool_precedes(w.nodes@[j], w.nodes@[i]) {
            pool_order_strict(w.nodes@[j], w.nodes@[i]);
            assert(pool_line(w.nodes@[j]));
            assert(pool_order(w.nodes@[i]) <= pool_order(w.nodes@[j]));
        }
    }
    assert(oldest_waiter(w, i));
}

/// Q39: a checked pool ticket is at an actual Start agenda item.
pub(super) proof fn pool_waiter_start(w: World, n: Node)
    requires node_checked(w, n), pool_line(n),
    ensures matches!(current_item(n), Some(Item::Start(_))), !(n.phase is Stopping || n.phase is Closed),
        n.effect.is_none(), !n.waiting_incident,
{
    reveal(node_checked);
    reveal(pool_line);
    match n.phase {
        Phase::Ingesting | Phase::Auditing | Phase::Unplanned | Phase::Planning { .. }
            | Phase::Ready(_) | Phase::Making | Phase::Checking | Phase::Reviewing
            | Phase::Publishing | Phase::Queued | Phase::Rebasing | Phase::AwaitingRob
            | Phase::Stopping(_) | Phase::Closed(_) => {},
    }
}

/// Q30/Q39 / #805: discharge this part of the real scheduler obstruction.
pub(super) proof fn no_outside_no_pool_line(w: World)
    requires world_checked(w), settled_world(w),
        forall|j: int| 0 <= j < w.nodes@.len() ==> !outside_available(w, j),
    ensures forall|j: int| 0 <= j < w.nodes@.len() ==> !pool_line(w.nodes@[j]),
{
    no_outside_no_pool(w);
    reveal(node_checked);
    reveal(outside_available);
    reveal(movable);
    reveal(oldest_waiter);
    reveal(pool_precedes);
    reveal(pool_line);
    world_checked_definition(w);
    settled_world_definition(w);
    if exists|j: int| 0 <= j < w.nodes@.len() && pool_line(w.nodes@[j]) {
        first_pool_waiter(w);
        let i = choose|i: int| 0 <= i < w.nodes@.len() && pool_line(w.nodes@[i]) && oldest_waiter(w, i);
        let n = w.nodes@[i];
        assert(node_checked(w, n));
        pool_waiter_start(w, n);
        assert(!outside_available(w, i));
        assert(!n.paused);
        assert(movable(w, i));
        assert(false);
    }
}

}
