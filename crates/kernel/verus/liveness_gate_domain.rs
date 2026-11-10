//! K5 G22 (Kani `gate_leaf_progress_w2/w3`): the gate-leaf domain and the trace invariants.
//! Logical definitions only. The domain contains every harness world of `gate_world_in_phase`
//! for W=2 and W=3: node 0 an unpaused Ready(Making) waiter with ticket t, node 1 its
//! overlapping Rebasing slot holder, nodes 2 and 3 Rebasing holders disjoint from node 0
//! (agenda position 1, Await) or younger Ready(Making) waiters. It enlarges that domain to any
//! gate width and any accepted four-node world in one repository with those node shapes.
use super::*;

verus! {
/// Acceptance plus the storage facts that scheduler::advance requires and preserves.
pub(super) open spec fn healthy(w: World) -> bool {
    checking::accepted(w) && scheduling_safe(w)
        && forall|j: int| 0 <= j < w.nodes@.len() ==> settle_shape(#[trigger] w.nodes@[j])
}

/// Node 0: the harness's unpaused, unobstructed Ready(Making) waiter with ticket t.
pub(super) open spec fn waiter_ok(w: World, t: u64) -> bool {
    w.nodes@[0].phase == Phase::Ready(Resume::Making)
        && advance_family_post::ready_unobstructed(w.nodes@[0])
        && w.nodes@[0].ticket == Some(t)
}

/// Never ahead of node 0 in the line: no ticket, or a later one.
pub(super) open spec fn behind(w: World, k: int, t: u64) -> bool {
    match w.nodes@[k].ticket { None => true, Some(x) => x > t }
}

/// A younger slot holder whose files are disjoint from node 0's (harness orientation).
pub(super) open spec fn younger_holder(w: World, k: int, t: u64) -> bool {
    w.nodes@[k].holds && footprint::disjoint(w, w.nodes@[0].footprint, w.nodes@[k].footprint)
        && behind(w, k, t)
}

/// The harness's younger holders: Rebasing, waiting for the rebase reply.
pub(super) open spec fn parked(w: World, k: int, t: u64) -> bool {
    younger_holder(w, k, t) && w.nodes@[k].phase == Phase::Rebasing
        && current_item(w.nodes@[k]) == Some(Item::Await) && !w.nodes@[k].waiting_incident
}

/// A younger waiter granted a slot in the first loop: Making, at PrepareWorktree.
pub(super) open spec fn granted(w: World, k: int, t: u64) -> bool {
    younger_holder(w, k, t) && w.nodes@[k].phase == Phase::Making && w.nodes@[k].pos == 0
        && !w.nodes@[k].paused
}

/// A younger unobstructed Ready(Making) waiter whose ticket is after node 0's.
pub(super) open spec fn younger_waiter(w: World, k: int, t: u64) -> bool {
    !w.nodes@[k].holds && advance_family_post::ready_unobstructed(w.nodes@[k])
        && w.nodes@[k].phase == Phase::Ready(Resume::Making)
        && w.nodes@[k].ticket.unwrap() > t
}

/// Nodes 2 and 3 in the starting world.
pub(super) open spec fn younger_start(w: World, k: int, t: u64) -> bool {
    w.nodes@[k].repo == w.nodes@[0].repo && (parked(w, k, t) || younger_waiter(w, k, t))
}

/// Nodes 2 and 3 before their visit in either loop.
pub(super) open spec fn younger_live(w: World, k: int, t: u64) -> bool {
    w.nodes@[k].repo == w.nodes@[0].repo
        && (parked(w, k, t) || granted(w, k, t) || younger_waiter(w, k, t))
}

/// Nodes 2 and 3 at any point after the teardown.
pub(super) open spec fn younger_any(w: World, k: int, t: u64) -> bool {
    w.nodes@[k].repo == w.nodes@[0].repo && (younger_holder(w, k, t) || younger_waiter(w, k, t))
}

/// Node 1: the harness's Rebasing blocker, with the reserves its Stop and teardown calls need
/// (gate_world_in_phase: last_cmd 0, generation 0, next_seq 0, no question, no dependencies).
pub(super) open spec fn blocker_ok(w: World) -> bool {
    w.nodes@[1].phase == Phase::Rebasing && w.nodes@[1].holds
        && w.nodes@[1].repo == w.nodes@[0].repo
        && !w.nodes@[1].waiting_incident && w.nodes@[1].effect.is_none()
        && w.nodes@[1].question.is_none() && w.nodes@[1].last_cmd == 0
        && arithmetic_consistent(w.nodes@[1]) && node_headroom(w.nodes@[1])
        && w.nodes@[1].generation as int + 8 <= u64::MAX as int
        && w.nodes@[1].next_seq as int + 8 <= u64::MAX as int
        && w.next_question as int + 2 <= u64::MAX as int
        && dependencies::prerequisites_delivered(w, NodeId(1))
}

/// The harness's starting worlds, enlarged as described in the module comment.
pub(super) open spec fn gate_start(w: World, t: u64) -> bool {
    w.nodes@.len() == 4 && healthy(w) && w.gates@.len() <= u64::MAX
        && w.nodes@[0].repo.0 < w.gates@.len()
        && waiter_ok(w, t) && blocker_ok(w)
        && !footprint::disjoint(w, w.nodes@[0].footprint, w.nodes@[1].footprint)
        && younger_start(w, 2, t) && younger_start(w, 3, t)
}

/// Trace invariant before the last conflicting holder (node 1) is torn down.
#[verifier::opaque]
pub(super) open spec fn pre_inv(w: World, t: u64) -> bool {
    w.nodes@.len() == 4 && healthy(w) && w.gates@.len() <= u64::MAX
        && w.nodes@[0].repo.0 < w.gates@.len()
        && waiter_ok(w, t) && blocker_ok(w)
        && younger_live(w, 2, t) && younger_live(w, 3, t)
}

/// Trace invariant after node 1 is closed: it holds nothing, and the remaining
/// holders leave room in node 0's gate.
#[verifier::opaque]
pub(super) open spec fn post_inv(w: World, t: u64) -> bool {
    w.nodes@.len() == 4 && healthy(w) && w.gates@.len() <= u64::MAX
        && w.nodes@[0].repo.0 < w.gates@.len()
        && waiter_ok(w, t)
        && w.nodes@[1].phase is Closed && !w.nodes@[1].holds
        && younger_any(w, 2, t) && younger_any(w, 3, t)
        && progress_gate_apply::holders(w, w.nodes@[0].repo, w.nodes@.len())
            < w.gates@[w.nodes@[0].repo.0 as int].w
}
}
