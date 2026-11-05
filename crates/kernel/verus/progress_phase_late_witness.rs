//! #806 A2: original-domain late-phase covers, including every terminal outcome.
use super::progress_phase::*;
use super::progress_phase_witness::original;
use super::*;
verus! {
/// #806 A2: the remaining six phase families; outcomes stay unrestricted.
pub(super) open spec fn phase_a2(phase: Phase) -> bool {
    matches!(phase, Phase::Publishing | Phase::Queued | Phase::Rebasing
        | Phase::AwaitingRob | Phase::Stopping(_) | Phase::Closed(_))
}
}

kernel_fn! {
/// Q30/Q39 / #806 A2: each late phase and every terminal outcome has a
/// consistent settled original-domain world. Closed retains an open node1.
fn world(phase: Phase) -> (result: World) [
    requires phase_a2(phase),
    ensures original(result), retirement::accepted(result), settled_world(result),
        result.nodes@[0].phase == phase,
        phase is Closed ==> !(result.nodes@[1].phase is Closed),
        exists|i: int| 0 <= i < 2 && !(result.nodes@[i].phase is Closed),
] proof [reveal(initial_node); reveal(retirement::accepted); reveal(retirement::node_rules);
    reveal(retirement::bounded);] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    first.phase = phase;
    first.paused = true;
    first.credit.ready = true;
    first.credit.start = 4;
    first.attempt = 1;
    first.footprint = Footprint::inline(1);
    first.head = Some(HeadId(1));
    first.checked = first.head;
    first.approved = first.head;
    second.phase = Phase::Closed(Outcome::Stopped);
    second.footprint = Footprint::inline(1);
    if phase.is_closed() {
        if phase == Phase::Closed(Outcome::Merged) {
            first.merged = first.head;
            first.had_pr = true;
            first.credit.merge_credited = true;
        }
        second.phase = Phase::Ingesting;
        second.paused = true;
    } else {
        first.effect = Some(InFlight { seq: 0, kind: EffectKind::StopRun, cancelling: true });
        first.next_seq = 1;
        match phase {
            Phase::Publishing | Phase::Rebasing => { first.holds = true; },
            Phase::Queued => {
                first.holds = true;
                first.pr_open = true;
                first.had_pr = true;
            },
            Phase::AwaitingRob => {
                first.question = Some(Question { id: 0, kind: QKind::NeedsSplitting,
                    cont: Cont::Nowhere, cost: 0, clarified: false });
            },
            Phase::Stopping(Outcome::Merged) => {
                first.merged = first.head;
                first.had_pr = true;
            },
            _ => {},
        }
    }
    let mut nodes = Vec::new();
    nodes.push(first);
    nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    gates.push(Gate { w: 1 });
    let w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 1,
        dependencies: Vec::new(), file_sets: Vec::new(), fail_fast: false };
    kernel_proof! {
        footprint::inline_is_valid(w, 1);
        assert forall|i: int| 0 <= i < 2 implies retirement::node_rules(w, w.nodes@[i]) by {
            assert(i == 0 || i == 1);
        }
        assert(retirement::accepted(w));
        assert forall|i: int| 0 <= i < w.nodes@.len() implies {
            let n = #[trigger] w.nodes@[i];
            n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
                && n.credit.start < 1099511627776u64 && n.credit.earned < 1099511627776u64
                && n.credit.added < 1099511627776u64 && n.credit.spent < 1099511627776u64
        } by { assert(i == 0 || i == 1); }
        assert(domain(w));
        assert(original(w));
        reveal(settled_world); reveal(movable);
        assert forall|i: int| 0 <= i < w.nodes@.len() implies !movable(w, i) by {
            assert(i == 0 || i == 1);
        }
    }
    scope(&w);
    kernel_proof! {
        reveal(progress_scope); reveal(storage_ordered);
        checked_reserves(w);
        zero_internal_height_definition(w);
        reveal_with_fuel(potential_prefix, 3);
        assert(potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)) == 0);
    }
    let settled = w.is_settled();
    assert!(settled);
    w
}
}

kernel_fn! {
/// #806 A2 cover: real witness and apply alone for the requested late phase.
/// The input phase preserves every Stopping/Closed outcome, and Closed
/// explicitly retains the original open-node1 requirement.
fn cover(phase: Phase) -> (result: (Ghost<World>, World, Event, u64, u64)) [
    requires phase_a2(phase),
    ensures original(result.0@), retirement::accepted(result.0@), settled_world(result.0@),
        result.0@.nodes@[0].phase == phase,
        phase is Closed ==> !(result.0@.nodes@[1].phase is Closed),
        witness_selection(result.0@, Some(result.2)), progress_input(result.0@, result.2),
        result.3 as nat == progress_measure(result.0@),
        result.4 as nat == progress_measure(result.1), result.4 < result.3,
] {
    let w = world(phase);
    kernel_snapshot! { before = w }
    let (after, event, old_measure, new_measure) = verify_apply(w);
    (before, after, event, old_measure, new_measure)
}
}
