//! Q48 / #806: all three original covers execute the real sequence at W2/W3.
use super::*;

// Actual worlds and per-younger-leaf results of the real sequence.
pub(super) type LeafCover = (World, World, bool, bool);
pub(super) type LeafCovers = (
    LeafCover,
    LeafCover,
    LeafCover,
    LeafCover,
    LeafCover,
    LeafCover,
);

verus! {
/// Q48 / #806: the exact original chained_waiter cover predicate.
pub(super) open spec fn chained_waiter(w: World) -> bool {
    w.nodes@[2].phase is Ready && w.nodes@[3].phase is Ready
        && retirement_gate::slot(w, 2)
        && match (w.nodes@[2].ticket, w.nodes@[3].ticket) {
            (Some(t), Some(younger)) => t < younger, _ => false,
        }
        && !footprint::disjoint(w, w.nodes@[0].footprint, w.nodes@[2].footprint)
        && footprint::disjoint(w, w.nodes@[0].footprint, w.nodes@[3].footprint)
        && footprint::disjoint(w, w.nodes@[2].footprint, w.nodes@[3].footprint)
}

/// Q48 / #806: exported full, passed and passed_chain covers over actual results.
pub(super) open spec fn cover_result(c: LeafCover, width: u8, case: u8) -> bool {
    progress_gate_leaf_sequence::input(c.0) && c.0.gates@[0].w == width
        && progress_gate_leaf_state::domain(c.1) && retirement_wide::accepted(c.1)
        && c.1.nodes@[0].holds && c.1.nodes@[0].ticket.is_none()
        && c.1.nodes@[1].phase is Closed
        && c.2 == (!c.0.nodes@[2].holds && c.1.nodes@[2].holds)
        && c.3 == (!c.0.nodes@[3].holds && c.1.nodes@[3].holds)
        && (case == 0 ==> retirement_wide::holders(c.0, RepoId(0), 4) == width)
        && (case == 1 ==> c.2 && c.1.nodes@[2].holds)
        && (case == 2 ==> chained_waiter(c.0) && c.3 && c.1.nodes@[3].holds)
}
}

verus! {
/// Q48 / #806: original-domain inputs which witness full, passed and passed_chain.
pub(super) open spec fn cover_input(w: World, width: u8, case: u8) -> bool {
    progress_gate_leaf_sequence::input(w) && w.gates@[0].w == width
        && (case == 0 ==> retirement_wide::holders(w, RepoId(0), 4) == width)
        && (case == 1 ==> w.nodes@[2].phase is Ready && !w.nodes@[2].holds
            && retirement_gate::slot(w, 2) && retirement_gate::line(w, 2))
        && (case == 2 ==> chained_waiter(w) && !w.nodes@[3].holds
            && progress_gate_leaf_state::updated(w, 2) == w.nodes@[2]
            && retirement_gate::slot(w, 3) && retirement_gate::line(w, 3))
}

/// Q48 / #806: the real observational copy preserves the original cover premises.
pub(super) proof fn copy_input(w: World, before: World, width: u8, case: u8)
    requires cover_input(w, width, case),
        before.nodes@ == w.nodes@, before.gates@ == w.gates@,
        before.file_sets@ == w.file_sets@, before.dependencies@ == w.dependencies@,
        before.pool_capacity == w.pool_capacity, before.next_ticket == w.next_ticket,
        before.next_question == w.next_question, before.fail_fast == w.fail_fast,
    ensures cover_input(before, width, case),
{
    reveal(retirement_wide::accepted); reveal(storage_ordered); reveal(footprint::ordered_table);
    reveal_with_fuel(potential_prefix, 5);
    assert forall|j: int| 0 <= j < 4 implies retirement::node_rules(before, before.nodes@[j]) by {
        retirement::node_frame(w, before, w.nodes@[j]);
    }
    assert forall|a: int, b: int| 0 <= a < 4 && 0 <= b < 4 implies
        footprint::disjoint(w, w.nodes@[a].footprint, w.nodes@[b].footprint)
            == footprint::disjoint(before, before.nodes@[a].footprint, before.nodes@[b].footprint) by {
        footprint::storage_frame(w, before, w.nodes@[a].footprint, w.nodes@[b].footprint);
    }
    assert forall|a: int, b: int| 0 <= a < b < 4 implies retirement_wide::pair(before, a, b) by {
        assert(retirement_wide::pair(w, a, b));
        footprint::storage_frame(w, before, w.nodes@[a].footprint, w.nodes@[b].footprint);
    }
    progress_gate_leaf_sequence::queries_frame(w, before);
    assert(progress_gate_leaf_state::domain(before));
    assert(retirement_wide::accepted(before));
    assert(progress_gate_leaf_state::low(before));
    assert(storage_ordered(before));
    assert(progress_gate_leaf_sequence::ordered(before));
    assert(progress_gate_leaf_sequence::input(before));
    assert(case == 0 ==> retirement_wide::holders(before, RepoId(0), 4) == width);
    assert(case == 1 ==> retirement_gate::slot(before, 2) && retirement_gate::line(before, 2));
    assert(case == 2 ==> chained_waiter(before));
    assert(case == 2 ==> progress_gate_leaf_state::updated(before, 2) == before.nodes@[2]) by {
        reveal(progress_gate_leaf_state::updated);
    }
}
}

kernel_fn! {
/// Q48 / #806: return actual input/output worlds and case-specific real results.
pub(super) fn leaf_cover(width: u8, case: u8) -> (result: LeafCover) [
    requires width == 2 || width == 3, case < 3,
    ensures cover_result(result, width, case),
] {
    let w = progress_gate_leaf_world::leaf_world(width, case);
    let before = snapshot_world(&w);
    kernel_proof! {
        assert(before.file_sets@ == w.file_sets@);
        copy_input(w, before, width, case);
    }
    let (after, passed2, passed3) = progress_gate_leaf_sequence::leaf_progress(w);
    assert!(after.nodes[0].holds);
    if case == 1 { assert!(passed2); assert!(after.nodes[2].holds); }
    if case == 2 { assert!(passed3); assert!(after.nodes[3].holds); }
    kernel_proof! {
        assert(progress_gate_leaf_sequence::input(before));
        assert(before.gates@[0].w == width);
        assert(progress_gate_leaf_state::domain(after));
        assert(retirement_wide::accepted(after));
        assert(after.nodes@[1].phase is Closed);
        assert(passed2 == (!before.nodes@[2].holds && after.nodes@[2].holds));
        assert(passed3 == (!before.nodes@[3].holds && after.nodes@[3].holds));
        assert(case == 0 ==> retirement_wide::holders(before, RepoId(0), 4) == width);
        assert(case == 2 ==> chained_waiter(before));
    }
    (before, after, passed2, passed3)
}
}

kernel_fn! {
/// Q48 / #806: all three original cover cases at W2 and W3, using real paths.
pub(super) fn all_leaf_covers() -> (result: LeafCovers) [
    ensures cover_result(result.0, 2, 0), cover_result(result.1, 3, 0),
        cover_result(result.2, 2, 1), cover_result(result.3, 3, 1),
        cover_result(result.4, 2, 2), cover_result(result.5, 3, 2),
] {
    (leaf_cover(2, 0), leaf_cover(3, 0),
     leaf_cover(2, 1), leaf_cover(3, 1),
     leaf_cover(2, 2), leaf_cover(3, 2))
}
}
