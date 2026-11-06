//! #806: original-domain settle covers for the seven remaining phase families.
use super::port_advance_phase::{same_world, verify_phase};
use super::port_advance_witness::original_domain;
use super::retirement::{accepted, bounded};
use super::*;

verus! {
/// #806 slice two: all remaining settle phases, including every terminal outcome.
pub(super) open spec fn selected(phase: Phase) -> bool {
    matches!(phase, Phase::Reviewing | Phase::Publishing | Phase::Queued
        | Phase::Rebasing | Phase::AwaitingRob | Phase::Stopping(_) | Phase::Closed(_))
}

/// #806: the exact moves argument of each remaining check_advance harness.
pub(super) open spec fn moves(phase: Phase) -> bool {
    matches!(phase, Phase::Reviewing | Phase::Publishing | Phase::Rebasing | Phase::Stopping(_))
}
}

kernel_fn! {
/// State-table late/terminal rows / #806: construct each original bounded phase.
/// Stopping is at its completed agenda, so every outcome executes real close_node.
pub(super) fn phase_world(phase: Phase) -> (result: World) [
    requires selected(phase),
    ensures original_domain(result), bounded(result), storage_ordered(result),
        result.nodes@[0].phase == phase, movable(result, 0) == moves(phase),
        phase is Stopping ==> result.nodes@[0].pos == 5,
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
    first.holds = matches!(phase, Phase::Reviewing | Phase::Publishing | Phase::Queued | Phase::Rebasing);
    if phase == Phase::Queued {
        first.pr_open = true;
        first.had_pr = true;
    }
    if phase == Phase::AwaitingRob {
        first.question = Some(Question { id: 0, kind: QKind::NeedsSplitting,
            cont: Cont::Nowhere, cost: 0, clarified: false });
    }
    if matches!(phase, Phase::Stopping(_)) {
        first.pos = 5;
    }
    if matches!(phase, Phase::Stopping(Outcome::Merged) | Phase::Closed(Outcome::Merged)) {
        first.merged = first.head;
        first.had_pr = true;
    }
    if phase == Phase::Closed(Outcome::Merged) {
        first.credit.merge_credited = true;
    }
    let mut second = Node::new(RepoId(1));
    second.phase = Phase::Closed(Outcome::Stopped);
    let mut nodes = Vec::new();
    nodes.push(first);
    nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    gates.push(Gate { w: 1 });
    let w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 1,
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
        assert(movable(w, 0) == moves(phase));
    }
    w
}
}

kernel_fn! {
/// #806 advance_reviewing/publishing/queued/rebasing/awaiting_rob/stopping/closed.
/// Every Outcome is symbolic; non-moving phases execute the real stuttering step.
pub(super) fn phase_cover(phase: Phase) -> (result: (World, Outbox, bool)) [
    requires selected(phase),
    ensures result.2 == moves(phase), accepted(result.0),
        !moves(phase) ==> result.0.nodes@[0].phase == phase && result.1.len == 0,
        forall|outcome: Outcome| phase == Phase::Stopping(outcome)
            ==> result.0.nodes@[0].phase == Phase::Closed(outcome),
] proof [reveal(port_advance_contracts::node_after);] {
    let w = phase_world(phase);
    let result = verify_phase(w);
    assert!(result.2 == matches!(phase,
        Phase::Reviewing | Phase::Publishing | Phase::Rebasing | Phase::Stopping(_)));
    result
}
}
