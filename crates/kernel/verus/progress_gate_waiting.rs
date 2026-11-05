//! Q30/Q39 / #806: waiting-only worlds have an internal move and no outside witness.
use super::progress_gate::*;
use super::progress_gate_classification::waiting;
use super::*;

verus! {
/// #806: no paused node, reply, merge, question, effect or prerequisite supplies an event.
proof fn waiting_no_outside(w: World)
    requires input(w), waiting(w),
    ensures forall|i: int| 0 <= i < 4 ==> !outside_available(w, i),
{
    reveal(initial_node); reveal(outside_available);
    assert forall|i: int| 0 <= i < 4 implies !outside_available(w, i) by {
        assert(leaf(w.nodes@[i]));
        dependencies::empty_delivered(w, NodeId(i as u64));
    }
}

/// #806: if waiting-only were settled, #805's actual scheduler obstruction gives a contradiction.
proof fn waiting_not_settled(w: World)
    requires input(w), world_checked(w), waiting(w),
    ensures !settled_world(w),
{
    waiting_no_outside(w);
    assert(!(w.nodes@[0].phase is Closed));
    if settled_world(w) {
        outside_branch_exists(w);
        assert(false);
    }
}
}

kernel_fn! {
/// #806: real witness returns None and the real is_settled probe returns false.
pub(super) fn verify_waiting(w: &World) -> (result: ()) [
    requires input(*w), waiting(*w),
    ensures !settled_world(*w),
] {
    kernel_proof! { input_scope(*w); }
    verify_checked(w);
    kernel_proof! {
        reveal(storage_ordered); checked_reserves(*w);
        waiting_no_outside(*w); waiting_not_settled(*w);
    }
    let event = w.witness();
    kernel_proof! { reveal(witness_selection); reveal(chosen_for_node); reveal(outside_available); }
    assert!(event.is_none());
    let settled = w.is_settled();
    assert!(!settled);
}
}
