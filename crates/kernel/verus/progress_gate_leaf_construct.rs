//! Q48 / #806: original constructor facts, isolated from executable assignments.
use super::progress_gate_leaf_witness::*;
use super::*;

verus! {
/// Q48 / #806: exact values assigned by the original four-node constructor.
pub(super) open spec fn cover_nodes(seed: Node, width: u8, case: u8) -> Seq<Node> {
    let first = Node { phase: Phase::Ready(Resume::Making),
        credit: Credit { ready: true, start: 4, ..seed.credit }, attempt: 1,
        footprint: footprint::inline_footprint(if case == 2 { 3u64 } else { 1u64 }),
        ticket: Some(1), ..seed };
    let blocker = Node { phase: Phase::Queued, ticket: None,
        footprint: footprint::inline_footprint(1), holds: true,
        head: Some(HeadId(1)), checked: Some(HeadId(1)), approved: Some(HeadId(1)),
        had_pr: true, pr_open: true, worktree: true, ..first };
    let second = if case == 0 { Node { footprint: footprint::inline_footprint(2), ..blocker } }
        else { Node { footprint: footprint::inline_footprint(2), ticket: Some(2), ..first } };
    let third = if case == 0 && width == 3 { Node { footprint: footprint::inline_footprint(4), ..blocker } }
        else { Node { footprint: footprint::inline_footprint(4), ticket: Some(3), ..first } };
    seq![first, blocker, second, third]
}

/// Q48 / #806: prove original construction and all three initial cover cases
/// from Node::new's actual contract and the exact executable field assignments.
pub(super) proof fn construction_valid(w: World, seed: Node, width: u8, case: u8)
    requires width == 2 || width == 3, case < 3, initial_node(seed, RepoId(0)),
        w.nodes@ == cover_nodes(seed, width, case), w.gates@ == seq![Gate { w: width }],
        w.pool_capacity == 4, w.next_ticket == 8, w.next_question == 0,
        w.file_sets@.len() == 0, w.dependencies@.len() == 0, !w.fail_fast,
    ensures cover_input(w, width, case), world_headroom(w), storage_ordered(w), scheduling_safe(w),
{

    reveal(progress_gate_leaf_state::updated); reveal(initial_node); reveal(retirement::node_rules); reveal(retirement_wide::accepted);
    reveal(storage_ordered); reveal(footprint::ordered_table); reveal(world_headroom);
    reveal_with_fuel(open_prefix, 5); reveal_with_fuel(potential_prefix, 5);
    reveal_with_fuel(retirement_gate::slot_count, 5); reveal(retirement_gate::slot_node);
        assert(1u64 & 1u64 != 0 && 1u64 & 2u64 == 0 && 1u64 & 4u64 == 0
            && 2u64 & 1u64 == 0 && 2u64 & 4u64 == 0 && 4u64 & 1u64 == 0
            && 4u64 & 2u64 == 0 && 3u64 & 1u64 != 0 && 1u64 & 3u64 != 0
            && 3u64 & 2u64 != 0 && 2u64 & 3u64 != 0 && 3u64 & 4u64 == 0
            && 4u64 & 3u64 == 0) by (bit_vector);
        footprint::inline_is_valid(w, 1); footprint::inline_is_valid(w, 2);
        footprint::inline_is_valid(w, 3); footprint::inline_is_valid(w, 4);
        footprint::inline_disjoint(w, 1, 1); footprint::inline_disjoint(w, 1, 2);
        footprint::inline_disjoint(w, 1, 4); footprint::inline_disjoint(w, 2, 1);
        footprint::inline_disjoint(w, 2, 4); footprint::inline_disjoint(w, 4, 1);
        footprint::inline_disjoint(w, 4, 2); footprint::inline_disjoint(w, 3, 1);
        footprint::inline_disjoint(w, 1, 3); footprint::inline_disjoint(w, 3, 2);
        footprint::inline_disjoint(w, 2, 3); footprint::inline_disjoint(w, 3, 4);
        footprint::inline_disjoint(w, 4, 3);
        assert forall|j: int| 0 <= j < 4 implies retirement::node_rules(w, w.nodes@[j]) by {}
        assert(retirement_wide::accepted(w));
        assert(progress_gate_leaf_sequence::input(w));
        if case == 1 {
            assert(retirement_gate::slot(w, 2));
            assert forall|j: int| 0 <= j < 4 && retirement_gate::older(w, 2, j)
                implies retirement_gate::eligible(w, 2, j) by { assert(j == 0); }
            assert(retirement_gate::line(w, 2));
        }
        if case == 2 {
            assert(retirement_gate::slot(w, 2));
            assert(retirement_gate::older(w, 2, 0));
            assert(!retirement_gate::eligible(w, 2, 0));
            if retirement_gate::line(w, 2) { assert(retirement_gate::eligible(w, 2, 0)); }
            assert(!retirement_gate::line(w, 2));
            assert(progress_gate_leaf_state::updated(w, 2) == w.nodes@[2]);
            assert(retirement_gate::overlapping_before(w, 2, 0, 2));
            assert(retirement_gate::slot(w, 3));
            assert forall|j: int| 0 <= j < 4 && retirement_gate::older(w, 3, j)
                implies retirement_gate::eligible(w, 3, j) by { assert(j == 0 || j == 2); }
            assert(retirement_gate::line(w, 3));
        }
}
}
