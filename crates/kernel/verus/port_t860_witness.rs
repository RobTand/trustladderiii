//! #860: executable, original-domain, settled covers of real apply.
use super::port_t860_contracts::*;
use super::*;

kernel_fn! {
/// Auditing / Q50 / #860: original two nodes, two gates, four-file inline domain.
/// Cases 0/1 are first/second audit, 2 stale, 3/4 schema rejection.
pub(super) fn world(case: u8, fail_fast: bool) -> (result: World) [
    requires case < 5,
    ensures progress_phase_witness::original(result), retirement::accepted(result),
        retirement::bounded(result), storage_ordered(result), settled_world(result),
        result.nodes@[0].phase == if case < 3 { Phase::Auditing }
            else { Phase::Ingesting },
        result.nodes@[0].run == Some(0), result.nodes@[0].generation == 0,
        result.nodes@[0].pos == 1, result.nodes@[0].job.is_none(),
        active(result.nodes@[0], 0), !active(result.nodes@[0], 1),
        result.nodes@[0].review_stop.is_none(), result.nodes@[0].question.is_none(),
        result.nodes@[0].run_incidents == 0,
        result.nodes@[0].audit_rounds == if case == 1 { 1u8 } else { 0u8 },
        result.nodes@[0].run_failures == if case == 4 { 1u8 } else { 0u8 },
        result.nodes@[0].credit.start == 4, result.next_question == 0, result.fail_fast == fail_fast,
        result.file_sets@.len() == 0,
] proof [reveal(initial_node); reveal(retirement::accepted); reveal(retirement::node_rules);
    reveal(retirement::bounded); reveal(storage_ordered); reveal(footprint::ordered_table);] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    first.phase = if case < 3 { Phase::Auditing }
        else { Phase::Ingesting };
    first.pos = 1;
    first.paused = true;
    first.credit.ready = true;
    first.credit.start = 4;
    first.footprint = Footprint::inline(1);
    first.run = Some(0);
    first.pool = true;
    first.audit_rounds = if case == 1 { 1u8 } else { 0u8 };
    first.run_failures = if case == 4 { 1u8 } else { 0u8 };
    second.phase = Phase::Closed(Outcome::Stopped);
    second.footprint = Footprint::inline(1);
    let mut nodes = Vec::new();
    nodes.push(first);
    nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    gates.push(Gate { w: 1 });
    let w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast };
    kernel_proof! {
        footprint::inline_is_valid(w, 1);
        assert forall|j: int| 0 <= j < 2 implies retirement::node_rules(w, w.nodes@[j]) by {
            assert(j == 0 || j == 1);
        }
        assert(retirement::accepted(w));
        assert forall|j: int| 0 <= j < 2 implies {
            let n = #[trigger] w.nodes@[j];
            n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
                && n.credit.start < 1099511627776u64 && n.credit.earned < 1099511627776u64
                && n.credit.added < 1099511627776u64 && n.credit.spent < 1099511627776u64
        } by { assert(j == 0 || j == 1); }
        reveal(settled_world); reveal(movable);
        assert forall|j: int| 0 <= j < 2 implies !movable(w, j) by { assert(j == 0 || j == 1); }
        assert(progress_phase_witness::original(w));
    }
    w
}
}

kernel_fn! {
/// #860: executable witnesses run real apply and return both worlds and outputs.
/// First disagreement re-ingests; second stops without a question, credit or Ask.
/// Stale generation stutters; wrong phase retries once then raises an incident.
pub(super) fn cover(case: u8, fail_fast: bool) -> (result: (World, World, Outbox)) [
    requires case < 5,
    ensures progress_phase_witness::original(result.0), retirement::accepted(result.0), settled_world(result.0),
        retirement::accepted(result.1), retirement::domain(result.1),
        result.0.fail_fast == fail_fast, result.1.next_question == result.0.next_question,
        result.1.nodes@[0].credit == result.0.nodes@[0].credit,
        retirement::notices_only(result.2),
        (case == 2 ==> result.1.nodes@ == result.0.nodes@),
        (case != 2 ==> result.1.nodes@ != result.0.nodes@),
        (case < 3 ==> empty_outbox(result.2)),
        (case == 0 ==> result.1.nodes@[0].phase == Phase::Ingesting
            && result.1.nodes@[0].review_stop.is_none() && result.1.nodes@[0].audit_rounds == 1),
        (case == 1 ==> result.1.nodes@[0].phase == Phase::Stopping(Outcome::Stopped)
            && result.1.nodes@[0].review_stop == Some(ReviewStop::AuditDisagreement)
            && result.1.nodes@[0].question.is_none() && result.1.nodes@[0].audit_rounds == 2),
        (case == 3 ==> result.1.nodes@[0].phase == Phase::Ingesting
            && result.1.nodes@[0].run_failures == 1 && result.1.nodes@[0].run.is_none()),
        (case == 4 ==> result.1.nodes@[0].waiting_incident
            && result.1.nodes@[0].run_failures == 0 && result.1.nodes@[0].run_incidents == 1),
] proof [reveal(reply_node); reveal(audit_node); reveal(failed_node); reveal(ended);
    reveal(retirement::stopped); reveal(retirement::incident);] {
    let w = world(case, fail_fast);
    let before = snapshot_world(&w);
    kernel_proof! {
        reveal(retirement::accepted);
        assert(before.file_sets@ == w.file_sets@);
        retirement::node_frame(w, before, w.nodes@[0]);
        retirement::node_frame(w, before, w.nodes@[1]);
        footprint::storage_frame(w, before, w.nodes@[0].footprint, w.nodes@[1].footprint);
        assert(retirement::accepted(before));
        reveal(settled_world);
        assert forall|j: int| 0 <= j < 2 implies !movable(before, j) by { movable_frame(w, before, j); }
        assert(settled_world(before));
    }
    let generation = if case == 2 { 1u64 } else { 0u64 };
    let (after, out) = port_t860_apply::verify_apply(w, generation);
    (before, after, out)
}
}
