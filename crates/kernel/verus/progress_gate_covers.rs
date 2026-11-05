//! #806: original W2/W3 reachability covers, including a nonzero actor swap.
use super::progress_gate::*;
use super::progress_gate_classification::*;
use super::progress_gate_fixture::*;
use super::*;

verus! {
/// Q30: original disjoint single-file footprints use the real inline checker predicate.
proof fn four_files(w: World)
    requires w.nodes@.len() == 4,
        w.nodes@[0].footprint == footprint::inline_footprint(1),
        w.nodes@[1].footprint == footprint::inline_footprint(2),
        w.nodes@[2].footprint == footprint::inline_footprint(4),
        w.nodes@[3].footprint == footprint::inline_footprint(8),
    ensures forall|a: int, b: int| 0 <= a < b < 4 ==>
        footprint::disjoint(w, w.nodes@[a].footprint, w.nodes@[b].footprint),
{
    footprint::inline_disjoint(w, 1, 2); footprint::inline_disjoint(w, 1, 4);
    footprint::inline_disjoint(w, 1, 8); footprint::inline_disjoint(w, 2, 4);
    footprint::inline_disjoint(w, 2, 8); footprint::inline_disjoint(w, 4, 8);
    assert(1u64 & 2u64 == 0 && 1u64 & 4u64 == 0 && 1u64 & 8u64 == 0
        && 2u64 & 4u64 == 0 && 2u64 & 8u64 == 0 && 4u64 & 8u64 == 0) by (bit_vector);
    assert forall|a: int, b: int| 0 <= a < b < 4 implies
        footprint::disjoint(w, w.nodes@[a].footprint, w.nodes@[b].footprint) by {
        assert(a == 0 || a == 1 || a == 2);
        assert(b == 1 || b == 2 || b == 3);
    }
}
}

kernel_fn! {
/// #806 covers at each width: no-witness waiters, all-Ready, all-Unplanned,
/// full gate with W admitted maker runs, and actor3 relocation before real completion.
fn original_covers(width: u8, case: u8) -> (result: ()) [
    requires width == 2 || width == 3, case < 4,
] proof [reveal(initial_node); reveal(accepted); reveal(retirement::node_rules);
    reveal_with_fuel(potential_prefix, 5);] {
    let a = make_leaf(if case == 0 { 0 } else if case == 2 { 2 } else { 3 }, 1, false, None);
    let b = make_leaf(if case == 0 { 0 } else if case == 2 { 2 } else { 3 }, 2, false, None);
    let c = make_leaf(if case == 0 { 0 } else if case == 2 && width == 3 { 2 } else { 3 }, 4, false, None);
    let d = make_leaf(if case == 0 { 0 } else if case == 3 { 2 } else { 3 }, 8, false, None);
    let w = assemble(width, a, b, c, d);
    kernel_proof! {
        four_files(w);
        footprint::inline_is_valid(w, 1); footprint::inline_is_valid(w, 2);
        footprint::inline_is_valid(w, 4); footprint::inline_is_valid(w, 8);
        assert forall|i: int| 0 <= i < 4 implies retirement::node_rules(w, w.nodes@[i]) by {
            assert(i == 0 || i == 1 || i == 2 || i == 3);
        }
        assert(input(w));
    }
    let event = verify_classification(&w);
    if case < 2 {
        assert!(event.is_none());
        kernel_proof! {
            assert(waiting(w));
            assert forall|i: int| 0 <= i < 4 implies
                w.nodes@[i].phase == if case == 0 { Phase::Ready(Resume::Making) } else { Phase::Unplanned } by {}
        }
        progress_gate_waiting::verify_waiting(&w);
    } else {
        assert!(event.is_some());
        kernel_proof! {
            if case == 2 { assert(holders(w, 4) == width && runs(w, 4) == width); }
        }
        let actor = if case == 3 { 3usize } else { 0usize };
        let swapped = progress_gate_permutation::swap_actor(w, actor);
        let _after = progress_gate_maker::verify_maker(swapped);
    }
}
}
