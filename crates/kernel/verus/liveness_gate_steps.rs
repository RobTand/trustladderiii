//! K5 G22: the step relations the gate-leaf trace composes, over (old(w), final(w), i, result).
//! `advanced` and `advanced_grants` restate scheduler::advance's exported contract on
//! fv-k5-int-dev-1110 (existing world frames, E3.1, E3.2, E4.1, E5.1, E5.3, E5.5, E5.6) clause for
//! clause. `advanced_requested` (Q1-Q4) and `torn_down` (E11) are NOT exported yet: they are the
//! clauses kernel/k5/liveness/opus/CONTRACT-REQUEST.md asks for. The exec binder over the real
//! harness calls is written once they are frozen.
use super::*;

verus! {
/// scheduler::advance: existing world frames and the E5.1/E5.3/E5.5 clauses acceptance needs.
#[verifier::opaque]
pub(super) open spec fn advanced(b: World, a: World, i: int, r: bool) -> bool {
    &&& 0 <= i < b.nodes@.len()
    &&& a.nodes@ == b.nodes@.update(i, a.nodes@[i])
    &&& a.next_question == b.next_question
    &&& a.gates@ == b.gates@
    &&& a.pool_capacity == b.pool_capacity
    &&& a.file_sets@ == b.file_sets@
    &&& a.dependencies@ == b.dependencies@
    &&& b.next_ticket <= a.next_ticket
    &&& ((forall|j: int| 0 <= j < b.nodes@.len() ==> settle_shape(#[trigger] b.nodes@[j]))
        ==> scheduling_safe(a)
            && (forall|j: int| 0 <= j < a.nodes@.len() ==> settle_shape(#[trigger] a.nodes@[j])))
    &&& (checking::accepted(b) ==> advance_family_post::step_rows(b, a, i))
    &&& (!b.nodes@[i].pool && a.nodes@[i].pool
        ==> pool_users(b.nodes@, b.nodes@.len()) < b.pool_capacity && oldest_waiter(b, i))
    &&& (!b.nodes@[i].holds && a.nodes@[i].holds
        ==> !b.nodes@[i].paused && scheduler::slot_spec(b, i)
            && a.nodes@[i].repo == b.nodes@[i].repo
            && a.nodes@[i].footprint == b.nodes@[i].footprint
            && a.nodes@[i].ticket.is_none()
            && a.nodes@[i].pool == b.nodes@[i].pool
            && a.nodes@[i].run == b.nodes@[i].run)
}

/// scheduler::advance: E3.1 grant, E3.2 stutter, E4.1 older-waiter reservation, E5.6 holds frame.
#[verifier::opaque]
pub(super) open spec fn advanced_grants(b: World, a: World, i: int, r: bool) -> bool {
    &&& (advance_family_post::ready_unobstructed(b.nodes@[i])
        && scheduler::slot_spec(b, i) && oldest_waiter(b, i)
        ==> r && a.nodes@[i].holds && a.nodes@[i].ticket.is_none())
    &&& (advance_family_post::ready_unobstructed(b.nodes@[i]) && !r
        ==> a.nodes@[i] == b.nodes@[i])
    &&& (advance_family_post::ready_unobstructed(b.nodes@[i])
        ==> forall|older: int| 0 <= older < b.nodes@.len()
            && b.nodes@[older].phase is Ready
            && b.nodes@[older].repo == b.nodes@[i].repo
            && b.nodes@[older].ticket.is_some()
            && b.nodes@[older].ticket.unwrap() < b.nodes@[i].ticket.unwrap()
            && scheduler::slot_spec(b, older)
            && !scheduler::overlapped_by_older(b, older, b.nodes@[older].ticket.unwrap())
            ==> !r && a.nodes@[i] == b.nodes@[i])
    &&& (b.nodes@[i].holds
        && (b.nodes@[i].phase == Phase::Making || b.nodes@[i].phase == Phase::Rebasing)
        ==> a.nodes@[i].holds)
}

/// REQUESTED from scheduler::advance (not exported yet).
/// Q1: first_in_line's K4c fairness clause carried through the grant: a granted Ready waiter
///     overlaps no older same-repository Ready waiter.
/// Q2: the world-level form of advance_family_post::slot_grant's granted branch.
/// Q3: an unpaused Making or Rebasing holder keeps its phase.
/// Q4: advance never changes `paused`.
#[verifier::opaque]
pub(super) open spec fn advanced_requested(b: World, a: World, i: int, r: bool) -> bool {
    &&& (advance_family_post::ready_unobstructed(b.nodes@[i]) && r
        ==> forall|j: int| 0 <= j < b.nodes@.len()
            && b.nodes@[j].phase is Ready
            && b.nodes@[j].repo == b.nodes@[i].repo
            && b.nodes@[j].ticket.is_some()
            && b.nodes@[j].ticket.unwrap() < b.nodes@[i].ticket.unwrap()
            ==> true)
    &&& (advance_family_post::ready_unobstructed(b.nodes@[i]) && r
        ==> a.nodes@[i].holds && a.nodes@[i].phase == resumed_phase(
            match b.nodes@[i].phase { Phase::Ready(x) => x, _ => Resume::Making }))
    &&& (b.nodes@[i].holds && !b.nodes@[i].paused
        && (b.nodes@[i].phase == Phase::Making || b.nodes@[i].phase == Phase::Rebasing)
        ==> a.nodes@[i].phase == b.nodes@[i].phase)
    &&& a.nodes@[i].paused == b.nodes@[i].paused
}

/// The harness's teardown of node 1: on_command(Stop), five rounds of advance(1) and a
/// Confirmed on_ack for any effect, then advance(1). The frames are what the existing per-call
/// contracts of on_command, on_ack and scheduler::advance give. The node 1 result and the
/// acceptance row are REQUESTED (CONTRACTS.md E11, OQ-7): no export states them yet.
#[verifier::opaque]
pub(super) open spec fn torn_down(b: World, a: World) -> bool {
    &&& a.nodes@.len() == b.nodes@.len()
    &&& forall|j: int| 0 <= j < b.nodes@.len() && j != 1 ==> #[trigger] a.nodes@[j] == b.nodes@[j]
    &&& a.gates@ == b.gates@
    &&& a.file_sets@ == b.file_sets@
    &&& a.nodes@[1].phase is Closed
    &&& !a.nodes@[1].holds
    &&& (liveness_gate_domain::healthy(b) ==> liveness_gate_domain::healthy(a))
}
}
