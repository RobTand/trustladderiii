//! State-table Queued merge / #806: actual apply and the complete checker.
use super::progress_gate_apply::*;
use super::*;

kernel_fn! {
/// #806 gate_merge_progress_w2/w3: one real apply strictly decreases #805's
/// progress_measure and passes the complete checker, for every positive width.
pub(super) fn verify_merge(mut w: World) -> (result: (World, Outbox)) [
    requires merge_input(w),
    ensures domain(result.0), accepted(result.0),
        progress_measure(result.0) < progress_measure(w),
        result.0.nodes@.len() == 4,
] proof [
    reveal(progress_gate_apply::accepted); reveal(gate_apply_checker_domain); reveal(gate_apply_checker_accepted); reveal(retirement::bounded);
    reveal(storage_ordered); reveal(empty_outbox); reveal(world_headroom); reveal_with_fuel(open_prefix, 5);
    reveal(progress_input); reveal(fresh_commands); reveal(chosen_for_node);
] {
    kernel_snapshot! { before = w }
    kernel_proof! { gate_reserves(w); }
    let checked_before = w.check();
    assert!(checked_before.is_ok());
    let head = w.nodes[0].head.unwrap_or(HeadId(0));
    let event = Event::Fact { node: NodeId(0), fact: Fact::PrMerged { head, checks_passed: true } };
    kernel_proof! {
        world_checked_definition(w);
        assert(chosen_for_node(w, 0, event));
        assert(witness_event(w, event));
        assert(progress_input(w, event));
    }
    let mut out = Outbox::new();
    apply(&mut w, event, &mut out);
    kernel_proof! { progress_gate_apply_merge_safe::merge_safe(before@, w, head); }
    let checked = w.check();
    assert!(checked.is_ok());
    (w, out)
}
}
