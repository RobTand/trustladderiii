//! Q48 / #806: all three original covers execute the real sequence at W2/W3.
use super::*;

kernel_fn! {
/// Q48: original-domain Node::new construction, four nodes, one gate, four files.
/// Case 0 starts full; case 1 permits a disjoint bypass; case 2 passes an older
/// waiter which is itself blocked by an overlapping oldest waiter.
fn leaf_cover(width: u8, case: u8) -> (result: ()) [
    requires width == 2 || width == 3, case < 3,
] proof [
    reveal(initial_node); reveal(retirement::node_rules); reveal(retirement_wide::accepted);
    reveal(storage_ordered); reveal(footprint::ordered_table); reveal(world_headroom);
    reveal_with_fuel(open_prefix, 5); reveal_with_fuel(potential_prefix, 5);
    reveal_with_fuel(retirement_gate::slot_count, 5); reveal(retirement_gate::slot_node);
] {
    let mut first = Node::new(RepoId(0));
    first.phase = Phase::Ready(Resume::Making);
    first.credit.ready = true;
    first.credit.start = 4;
    first.attempt = 1;
    first.footprint = Footprint::inline(if case == 2 { 3 } else { 1 });
    first.ticket = Some(1);
    let mut blocker = first;
    blocker.phase = Phase::Queued;
    blocker.ticket = None;
    blocker.footprint = Footprint::inline(1);
    blocker.holds = true;
    blocker.head = Some(HeadId(1));
    blocker.checked = blocker.head;
    blocker.approved = blocker.head;
    blocker.had_pr = true;
    blocker.pr_open = true;
    blocker.worktree = true;
    let mut second = first;
    second.footprint = Footprint::inline(2);
    second.ticket = Some(2);
    let mut third = first;
    third.footprint = Footprint::inline(4);
    third.ticket = Some(3);
    if case == 0 {
        second = blocker;
        second.footprint = Footprint::inline(2);
        if width == 3 {
            third = blocker;
            third.footprint = Footprint::inline(4);
        }
    }
    let mut nodes = Vec::new();
    nodes.push(first); nodes.push(blocker); nodes.push(second); nodes.push(third);
    let mut gates = Vec::new();
    gates.push(Gate { w: width });
    let w = World { nodes, gates, pool_capacity: 4, next_ticket: 8, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    kernel_proof! {
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
    let checked = w.check();
    assert!(checked.is_ok());
    if case == 0 {
        kernel_proof! { assert(retirement_wide::holders(w, RepoId(0), 4) == width); }
    }
    if case == 2 {
        // The exact original chained_waiter condition, evaluated with real queries.
        assert!(matches!(w.nodes[2].phase, Phase::Ready(_)));
        assert!(matches!(w.nodes[3].phase, Phase::Ready(_)));
        assert!(slot_free(&w, 2));
        assert!(w.nodes[2].ticket.unwrap() < w.nodes[3].ticket.unwrap());
        assert!(!w.footprints_disjoint(w.nodes[0].footprint, w.nodes[2].footprint));
        assert!(w.footprints_disjoint(w.nodes[0].footprint, w.nodes[3].footprint));
        assert!(w.footprints_disjoint(w.nodes[2].footprint, w.nodes[3].footprint));
    }
    let (after, passed2, passed3) = progress_gate_leaf_sequence::leaf_progress(w);
    assert!(after.nodes[0].holds);
    if case == 1 { assert!(passed2); assert!(after.nodes[2].holds); }
    if case == 2 { assert!(passed3); assert!(after.nodes[3].holds); }
}
}

kernel_fn! {
/// Q48 / #806: all three original cover cases at W2 and W3, using real paths.
fn all_leaf_covers() -> (result: ()) [] {
    leaf_cover(2, 0); leaf_cover(3, 0);
    leaf_cover(2, 1); leaf_cover(3, 1);
    leaf_cover(2, 2); leaf_cover(3, 2);
}
}
