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
    kernel_snapshot! { before_out = out }
    let changed = advance(&mut w, i, &mut out);
    kernel_proof! {
        let context = choose|context: AdvanceContext| context.node == i
            && changed == advance_enabled(before@.nodes@[i as int], context)
            && port_advance_contracts::exact(before@.nodes@[i as int], w.nodes@[i as int],
                before@.next_ticket, w.next_ticket, context, before_out@, out);
        progress_gate_leaf_adapter::local_details(before@.nodes@[i as int], w.nodes@[i as int],
            before@.next_ticket, w.next_ticket, context, before_out@, out);
        if before@.nodes@[i as int].phase == Phase::Queued { assert(!changed); }
        if before@.nodes@[i as int].phase is Ready {
            retirement_gate_bridge::queries(before@, i as int);
            retirement_gate_bridge::post(before@.nodes@[i as int], w.nodes@[i as int],
                before@.next_ticket, w.next_ticket,
                retirement_gate::slot(before@, i as int) && retirement_gate::line(before@, i as int),
                changed, before_out@, out);
        }
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
