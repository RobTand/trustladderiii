//! Q48 / #806: independently verified real scheduler and complete-checker calls.
use super::progress_gate_leaf_state::*;
use super::*;

kernel_fn! {
/// Q48 / #806: advance the original leaf through the real scheduler and checker.
pub(super) fn advance_leaf(mut w: World, i: usize) -> (result: World) [
    requires domain(w), retirement_wide::accepted(w), low(w), storage_ordered(w),
        i < 4, tracked(w.nodes@[i as int]),
        w.nodes@[i as int].generation <= 1000, w.nodes@[i as int].next_seq <= 1000,
        w.nodes@[i as int].phase is Ready ==> w.nodes@[i as int].ticket.is_some(),
    ensures domain(result), retirement_wide::accepted(result), storage_ordered(result),
        frame(w, result, i as int), result.nodes@[i as int] == updated(w, i as int),
        low(result), result.nodes@[i as int].generation <= w.nodes@[i as int].generation + 1,
        result.nodes@[i as int].next_seq <= w.nodes@[i as int].next_seq + 1,
] {
    kernel_snapshot! { before = w }
    kernel_proof! { advance_input(w, i as int); }
    let mut out = Outbox::new();
    let changed = advance(&mut w, i, &mut out);
    kernel_proof! {
        if before@.nodes@[i as int].phase == Phase::Queued { assert(!changed); }
        advance_result(before@, w, i as int, changed);
        update_safe(before@, w, i as int);
        advance_input(w, i as int);
        reveal(retirement_wide_domain); reveal(retirement_wide_accepted);
    }
    let checked = w.check();
    assert!(checked.is_ok());
    w
}
}
