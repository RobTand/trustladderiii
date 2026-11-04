// Included in the kernel module so proofs use the production world's storage.
#[cfg(verus_keep_ghost)]
verus! {
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
