//! Q30/Q48 / #806: all ten original W2/W3 grant covers over real advance.
use super::*;

kernel_fn! {
/// Q48: each width has a last-slot, capacity, older-runnable, disjoint-bypass
/// and overlapping-older witness. Bounds match gate_world: four nodes/files.
fn grant_covers(width: u8, case: u8) -> (result: ()) [
    requires width == 2 || width == 3, case < 5,
] proof [
    reveal(initial_node); reveal(retirement::node_rules); reveal(retirement_wide::accepted);
    reveal(retirement::bounded); reveal(storage_ordered); reveal(footprint::ordered_table);
    reveal_with_fuel(potential_prefix, 5); reveal_with_fuel(retirement_gate::slot_count, 5);
    reveal(retirement_gate::slot_node);
] {
    let mut first = Node::new(RepoId(0));
    first.phase = Phase::Ready(Resume::Making);
    first.credit.ready = true;
    first.credit.start = 4;
    first.attempt = 1;
    first.footprint = Footprint::inline(1);
    first.ticket = Some(2);
    let mut second = first;
    second.footprint = Footprint::inline(if case == 4 { 3 } else { 2 });
    second.ticket = if case >= 2 { Some(1) } else { None };
    let mut third = first;
    third.ticket = None;
    third.footprint = Footprint::inline(if case >= 3 { 2 } else { 4 });
    let mut fourth = first;
    fourth.ticket = None;
    fourth.footprint = Footprint::inline(8);
    if case < 2 {
        second.phase = Phase::Queued;
        second.holds = true;
        second.had_pr = true;
        second.pr_open = true;
        second.worktree = true;
        second.head = Some(HeadId(1));
        second.checked = second.head;
        second.approved = second.head;
    }
    if case == 1 || case >= 3 || (case == 0 && width == 3) {
        third.phase = Phase::Queued;
        third.holds = true;
        third.had_pr = true;
        third.pr_open = true;
        third.worktree = true;
        third.head = Some(HeadId(1));
        third.checked = third.head;
        third.approved = third.head;
    } else {
        third.phase = Phase::Unplanned;
    }
    if case == 1 && width == 3 {
        fourth.phase = Phase::Queued;
        fourth.holds = true;
        fourth.had_pr = true;
        fourth.pr_open = true;
        fourth.worktree = true;
        fourth.head = Some(HeadId(1));
        fourth.checked = fourth.head;
        fourth.approved = fourth.head;
    } else {
        fourth.phase = Phase::Unplanned;
    }
    let mut nodes = Vec::new();
    nodes.push(first); nodes.push(second); nodes.push(third); nodes.push(fourth);
    let mut gates = Vec::new();
    gates.push(Gate { w: width });
    let before = World { nodes, gates, pool_capacity: 4, next_ticket: 8, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    kernel_proof! {
        assert(1u64 & 2u64 == 0 && 1u64 & 4u64 == 0 && 1u64 & 8u64 == 0
            && 2u64 & 4u64 == 0 && 2u64 & 8u64 == 0 && 4u64 & 8u64 == 0
            && 2u64 & 1u64 == 0 && 4u64 & 1u64 == 0 && 8u64 & 1u64 == 0
            && 4u64 & 2u64 == 0 && 8u64 & 2u64 == 0 && 8u64 & 4u64 == 0
            && 2u64 & 2u64 != 0 && 3u64 & 1u64 != 0 && 2u64 & 3u64 != 0) by (bit_vector);
        footprint::inline_is_valid(before, 1); footprint::inline_is_valid(before, 2);
        footprint::inline_is_valid(before, 3); footprint::inline_is_valid(before, 4);
        footprint::inline_is_valid(before, 8);
        footprint::inline_disjoint(before, 1, 2); footprint::inline_disjoint(before, 1, 4);
        footprint::inline_disjoint(before, 1, 8); footprint::inline_disjoint(before, 2, 4);
        footprint::inline_disjoint(before, 2, 8); footprint::inline_disjoint(before, 4, 8);
        footprint::inline_disjoint(before, 2, 2); footprint::inline_disjoint(before, 3, 1);
        footprint::inline_disjoint(before, 2, 3);
        footprint::inline_disjoint(before, 2, 1); footprint::inline_disjoint(before, 4, 1);
        footprint::inline_disjoint(before, 8, 1); footprint::inline_disjoint(before, 4, 2);
        footprint::inline_disjoint(before, 8, 2); footprint::inline_disjoint(before, 8, 4);
        assert(retirement_gate::grant_input(before));
        if case == 0 || case == 3 {
            assert(retirement_gate::slot_count(before, 0, 4) < width);
            assert forall|j: int| 0 < j < 4 && before.nodes@[j].holds
                implies footprint::disjoint(before, before.nodes@[j].footprint, before.nodes@[0].footprint) by {
                assert(j == 1 || j == 2 || j == 3);
            }
            assert(retirement_gate::slot(before, 0));
            assert forall|j: int| 0 <= j < 4 && retirement_gate::older(before, 0, j)
                implies retirement_gate::eligible(before, 0, j) by {
                assert(j == 1);
            }
            assert(retirement_gate::line(before, 0));
        }
        assert forall|j: int| 0 <= j < 4 implies retirement::node_rules(before, before.nodes@[j]) by {}
    }
    kernel_snapshot! { original = before }
    let (after, _, _) = retirement_gate::verify_grant(before);
    kernel_proof! {
        if case == 0 {
            assert(retirement_gate::slot_count(original@, 0, 4) == width - 1);
            assert(after.nodes@[0].holds);
        } else if case == 1 {
            assert(original@.nodes@[0].ticket.is_some() && !original@.nodes@[0].paused);
            assert(!retirement_gate::slot(original@, 0));
            assert(!after.nodes@[0].holds);
        } else if case == 2 {
            assert(retirement_gate::slot(original@, 0));
            assert(retirement_gate::older(original@, 0, 1));
            assert(retirement_gate::slot(original@, 1));
            assert(retirement_gate::line(original@, 1));
            assert(!retirement_gate::line(original@, 0));
            assert(!after.nodes@[0].holds);
        } else if case == 3 {
            assert(retirement_gate::older(original@, 0, 1));
            assert(!retirement_gate::slot(original@, 1));
            assert(after.nodes@[0].holds);
        } else {
            assert(retirement_gate::older(original@, 0, 1));
            assert(!retirement_gate::slot(original@, 1));
            assert(!footprint::disjoint(original@, original@.nodes@[1].footprint, original@.nodes@[0].footprint));
            assert(retirement_gate::slot(original@, 0));
            assert(!after.nodes@[0].holds);
        }
    }
}
}
