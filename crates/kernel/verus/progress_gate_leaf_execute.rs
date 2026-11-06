//! Q48 / #806: the unchanged real sequence, independently gated from its lemmas.
use super::progress_gate_leaf_sequence::*;
use super::progress_gate_leaf_state::*;
use super::*;

kernel_fn! {
/// Q48 / #806: original advance(2), advance(3), real Stop(1), five successful
/// cleanup rounds, advance(1), advance(2), advance(3), advance(0), complete check.
pub(super) fn leaf_progress(mut w: World) -> (result: (World, bool, bool)) [
    requires input(w),
    ensures domain(result.0), retirement_wide::accepted(result.0), result.0.nodes@[0].holds,
        result.0.nodes@[0].ticket.is_none(), result.0.nodes@[1].phase is Closed,
        result.1 == (!w.nodes@[2].holds && result.0.nodes@[2].holds),
        result.2 == (!w.nodes@[3].holds && result.0.nodes@[3].holds),
        w.nodes@[2].phase is Ready && retirement_gate::slot(w, 2) && retirement_gate::line(w, 2)
            ==> result.0.nodes@[2].holds,
        updated(w, 2) == w.nodes@[2] && w.nodes@[3].phase is Ready
            && retirement_gate::slot(w, 3) && retirement_gate::line(w, 3) ==> result.0.nodes@[3].holds,
        result.0.nodes@[2].holds ==> footprint::disjoint(result.0, result.0.nodes@[0].footprint, result.0.nodes@[2].footprint),
        result.0.nodes@[3].holds ==> footprint::disjoint(result.0, result.0.nodes@[0].footprint, result.0.nodes@[3].footprint),
] proof [
    reveal(storage_ordered);
    reveal_with_fuel(open_prefix, 5);
] {
    kernel_snapshot! { original = w }
    kernel_proof! { input_details(w); }
    let held_before2 = w.nodes[2].holds;
    w = advance_leaf(w, 2);
    let passed2 = !held_before2 && w.nodes[2].holds;
    kernel_proof! {
        visit_details(original@, w, 2);
        preserve_order(original@, w, 2);
        if updated(original@, 2) == original@.nodes@[2] {
            assert(w.nodes@ =~= original@.nodes@);
            queries_frame(original@, w);
        }
    }
    kernel_snapshot! { visited2 = w }
    let held_before3 = w.nodes[3].holds;
    w = advance_leaf(w, 3);
    let passed3 = !held_before3 && w.nodes[3].holds;
    kernel_proof! { visit_details(visited2@, w, 3); preserve_order(visited2@, w, 3); }
    kernel_snapshot! { blocked = w }
    kernel_proof! { assert(w.nodes@[0].ticket == original@.nodes@[0].ticket); }
    w = progress_gate_leaf_teardown::teardown(w);
    kernel_proof! { released(blocked@, w); }
    kernel_proof! {
        assert(scheduling_safe(w)) by {
            reveal(storage_ordered); reveal(retirement_wide::accepted); reveal(retirement::node_rules);
            reveal_with_fuel(open_prefix, 5);
        }
    }
    kernel_proof! { retirement_gate_bridge::queries(w, 0); }
    assert!(slot_free(&w, 0));
    kernel_snapshot! { freed = w }
    let held2 = w.nodes[2].holds;
    w = advance_leaf(w, 2);
    kernel_proof! { no_steal_step(freed@, w, 2); }
    assert!(w.nodes[2].holds == held2);
    kernel_snapshot! { visited_again2 = w }
    let held3 = w.nodes[3].holds;
    w = advance_leaf(w, 3);
    kernel_proof! { no_steal_step(visited_again2@, w, 3); }
    assert!(w.nodes[3].holds == held3);
    kernel_snapshot! { grant_before = w }
    w = advance_leaf(w, 0);
    kernel_proof! {
        grant_result(grant_before@, w);
    }
    assert!(w.nodes[0].holds && w.nodes[0].ticket.is_none());
    (w, passed2, passed3)
}
}
