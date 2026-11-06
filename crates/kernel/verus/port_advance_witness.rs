//! #806: executable original-domain covers for every slice-one phase parameter.
use super::*;
use super::port_advance_phase::*;
use super::retirement::{accepted, bounded, domain};

verus! {
/// #806: the exact optional-head bound of proofs.rs::small.
pub(super) open spec fn small_head(head: Option<HeadId>) -> bool {
    match head { None => true, Some(head) => head.0 < 8 }
}

/// #806: the original LIMIT/CREDIT_LIMIT bounds, including unused stored fields.
pub(super) open spec fn small(n: Node) -> bool {
    n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
        && n.last_cmd < 1099511627776u64 && n.repo.0 < 2
        && small_head(n.proposed) && small_head(n.head) && small_head(n.checked)
        && small_head(n.approved) && small_head(n.merged)
        && n.credit.start < 1073741824u64 && n.credit.earned < 1073741824u64
        && n.credit.added < 1073741824u64 && n.credit.spent < 1073741824u64
        && (match n.ticket { None => true, Some(t) => t < 1099511627776u64 })
        && (match n.run { None => true, Some(g) => g < 1099511627776u64 })
        && (match n.job { None => true, Some(g) => g < 1099511627776u64 })
        && (match n.effect { None => true, Some(e) => e.seq < 1099511627776u64 })
        && (match n.question { None => true, Some(q) => q.id < 1099511627776u64 })
}

/// #806: retain every assumption of any_world for the executable cover constructors.
pub(super) open spec fn original_domain(w: World) -> bool {
    domain(w) && accepted(w) && w.file_sets@.len() == 0
        && w.pool_capacity <= 2 && w.gates@[0].w <= 3 && w.gates@[1].w <= 3
        && w.next_ticket < 1099511627776u64 && w.next_question < 1099511627776u64
        && small(w.nodes@[0]) && small(w.nodes@[1]) && port_advance_world::inline_nodes(w)
}
}

kernel_fn! {
/// State-table slice-one rows / #806: construct the original two-request, two-repo world.
/// Phase is symbolic, so both Planning flags and all five Resume values are covered.
pub(super) fn phase_world(phase: Phase) -> (result: World) [
    requires slice_one(phase),
    ensures original_domain(result), bounded(result), storage_ordered(result),
        result.nodes@[0].phase == phase, movable(result, 0),
] proof [
    reveal(initial_node); reveal(accepted); reveal(retirement::node_rules);
    reveal(node_checked); reveal(ready_checked); reveal(bounded);
    reveal(storage_ordered); reveal(footprint::ordered_table);
    reveal_with_fuel(potential_prefix, 3);
] {
    let mut first = Node::new(RepoId(0));
    first.phase = phase;
    first.credit.ready = true;
    first.credit.start = 4;
    first.attempt = 1;
    first.footprint = Footprint::inline(1);
    first.head = Some(HeadId(1));
    first.checked = first.head;
    first.approved = first.head;
    first.holds = matches!(phase, Phase::Making | Phase::Checking);
    let mut second = Node::new(RepoId(1));
    second.phase = Phase::Closed(Outcome::Stopped);
    let mut nodes = Vec::new();
    nodes.push(first);
    nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    gates.push(Gate { w: 1 });
    let w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    kernel_proof! {
        footprint::inline_is_valid(w, 0);
        footprint::inline_is_valid(w, 1);
        dependencies::delivered_definition(w.dependencies@, NodeId(0));
        assert forall|i: int| 0 <= i < w.nodes@.len() implies {
            let n = #[trigger] w.nodes@[i];
            n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
                && n.credit.start < 1099511627776u64 && n.credit.earned < 1099511627776u64
                && n.credit.added < 1099511627776u64 && n.credit.spent < 1099511627776u64
        } by { assert(i == 0 || i == 1); }
        assert forall|i: int| 0 <= i < w.nodes@.len() implies {
            let n = #[trigger] w.nodes@[i];
            exists|bits: u64| bits < 16 && n.footprint == footprint::inline_footprint(bits)
        } by { assert(i == 0 || i == 1); }
        assert(retirement::node_rules(w, w.nodes@[0]));
        assert(retirement::node_rules(w, w.nodes@[1]));
        assert(accepted(w));
        assert(movable(w, 0));
    }
    w
}
}

kernel_fn! {
/// #806 advance_ingesting/auditing/unplanned/planning/ready/making/checking covers.
/// Execute real advance on an original-domain world for every phase parameter.
pub(super) fn phase_cover(phase: Phase) -> (result: (World, Outbox, bool)) [
    requires slice_one(phase),
    ensures result.2, accepted(result.0),
] {
    let w = phase_world(phase);
    let result = verify_phase(w);
    assert!(result.2);
    result
}
}
