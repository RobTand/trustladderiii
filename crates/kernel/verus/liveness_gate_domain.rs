//! K5 G22 (Kani `gate_leaf_progress_w2/w3`): the gate-leaf domain and the trace invariants.
//! Logical definitions only. The domain contains every harness world of `gate_world_in_phase`
//! for W=2 and W=3: node 0 an unpaused Ready(Making) waiter with ticket t, node 1 its
//! overlapping Rebasing slot holder, nodes 2 and 3 slot holders disjoint from node 0 or younger
//! Ready waiters. It enlarges that domain to any gate width, any accepted four-node world in one
//! repository, Making as well as Rebasing holders, and Ready(Rebasing) as well as Ready(Making)
//! younger waiters.
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

/// A younger unpaused slot holder whose files are disjoint from node 0's
/// (the harness's orientation: node 0's footprint first).
pub(super) open spec fn younger_holder(w: World, k: int) -> bool {
    w.nodes@[k].holds && !w.nodes@[k].paused
        && (w.nodes@[k].phase == Phase::Making || w.nodes@[k].phase == Phase::Rebasing)
        && footprint::disjoint(w, w.nodes@[0].footprint, w.nodes@[k].footprint)
}

/// A younger unobstructed Ready waiter whose ticket is after node 0's.
pub(super) open spec fn younger_waiter(w: World, k: int, t: u64) -> bool {
    !w.nodes@[k].holds && advance_family_post::ready_unobstructed(w.nodes@[k])
        && (w.nodes@[k].phase == Phase::Ready(Resume::Making)
            || w.nodes@[k].phase == Phase::Ready(Resume::Rebasing))
        && w.nodes@[k].ticket.unwrap() > t
}

/// Nodes 2 and 3: same repository as node 0, a disjoint holder or a younger waiter.
pub(super) open spec fn younger_ok(w: World, k: int, t: u64) -> bool {
    w.nodes@[k].repo == w.nodes@[0].repo
        && (younger_holder(w, k) || younger_waiter(w, k, t))
}

/// The harness's starting worlds, enlarged as described in the module comment.
pub(super) open spec fn gate_start(w: World, t: u64) -> bool {
    w.nodes@.len() == 4 && healthy(w) && w.gates@.len() <= u64::MAX
        && w.nodes@[0].repo.0 < w.gates@.len()
        && waiter_ok(w, t)
        && w.nodes@[1].phase == Phase::Rebasing && w.nodes@[1].holds
        && w.nodes@[1].repo == w.nodes@[0].repo
        && !footprint::disjoint(w, w.nodes@[0].footprint, w.nodes@[1].footprint)
        && younger_ok(w, 2, t) && younger_ok(w, 3, t)
}

/// Trace invariant before the last conflicting holder (node 1) is torn down.
#[verifier::opaque]
pub(super) open spec fn pre_inv(w: World, t: u64) -> bool {
    w.nodes@.len() == 4 && healthy(w) && w.gates@.len() <= u64::MAX
        && w.nodes@[0].repo.0 < w.gates@.len()
        && waiter_ok(w, t)
        && w.nodes@[1].holds && w.nodes@[1].repo == w.nodes@[0].repo
        && younger_ok(w, 2, t) && younger_ok(w, 3, t)
}

/// Trace invariant after node 1 is closed: it holds nothing, and the remaining
/// holders leave room in node 0's gate.
#[verifier::opaque]
pub(super) open spec fn post_inv(w: World, t: u64) -> bool {
    w.nodes@.len() == 4 && healthy(w) && w.gates@.len() <= u64::MAX
        && w.nodes@[0].repo.0 < w.gates@.len()
        && waiter_ok(w, t)
        && w.nodes@[1].phase is Closed && !w.nodes@[1].holds
        && younger_ok(w, 2, t) && younger_ok(w, 3, t)
        && progress_gate_apply::holders(w, w.nodes@[0].repo, w.nodes@.len())
            < w.gates@[w.nodes@[0].repo.0 as int].w
}
}
