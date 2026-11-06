//! #806: proof-only consequences of exact production retry and incident updates.
use super::*;
verus! {
/// #806 / Q50: the incident guard stutters exactly.
pub(super) proof fn upstream_noop(n: Node)
    requires n.phase != Phase::Queued || n.waiting_incident,
    ensures retirement::fact_node(n, Fact::UpstreamStalled) == n,
{ reveal(retirement::fact_node); }
/// #806 / Q50: the actual saturated counter update precedes the real incident helper.
pub(super) proof fn upstream_result(n: Node)
    requires n.phase == Phase::Queued, !n.waiting_incident,
    ensures retirement::fact_node(n, Fact::UpstreamStalled) == {
        let count = if n.run_incidents < 2 { (n.run_incidents + 1) as u8 } else { 2u8 };
        retirement::incident(Node { run_incidents: count, ..n }, count >= 2)
    },
{ reveal(retirement::fact_node); }
/// #806 / Q54: exact rung fields imply every original retry bound and frame.
pub(super) proof fn rung_bounds(before: World, after: World, i: int, step: ReviewStop,
    out_before: usize, out_after: usize)
    requires 0 <= i < before.nodes@.len(), node_headroom(before.nodes@[i]),
        after.nodes@[i] == retirement_retry::rung(before.nodes@[i], before.fail_fast, before.next_question, step),
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != i ==> after.nodes@[j] == before.nodes@[j],
        after.next_ticket == before.next_ticket, after.next_question as int <= before.next_question as int + 1,
        after.gates@ == before.gates@, after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast, after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@, out_after as int <= out_before as int + 1,
    ensures retry_bounds(before, after, i, out_before, out_after),
{
    reveal(retry_bounds); reveal(retirement_retry::rung); reveal(retirement::stopped);
}
/// #806 / Q50: exact recheck fields imply every original retry bound and frame.
pub(super) proof fn recheck_bounds(before: World, after: World, i: int, target: Phase,
    out_before: usize, out_after: usize)
    requires 0 <= i < before.nodes@.len(), node_headroom(before.nodes@[i]), !(target is Closed),
        after.nodes@[i] == retirement_retry::rechecked(before.nodes@[i], target, before.next_question),
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != i ==> after.nodes@[j] == before.nodes@[j],
        after.next_ticket == before.next_ticket, after.next_question as int <= before.next_question as int + 1,
        after.gates@ == before.gates@, after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast, after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@, out_after as int <= out_before as int + 1,
    ensures retry_bounds(before, after, i, out_before, out_after),
{
    reveal(retry_bounds); reveal(retirement_retry::rechecked);
    reveal(retirement_retry::recheck_core); reveal(retirement_retry::continued);
}
/// #805 / Q50: the saturated allowance increment does not affect the old distance.
pub(super) proof fn rechecked_distance(n: Node, id: u64)
    requires ordinary_await(n), n.phase == Phase::Rebasing, node_headroom(n),
    ensures distance(retirement_retry::rechecked(n, Phase::Checking, id)) < distance(n),
{
    reveal(retirement_retry::rechecked);
    let m = Node { rechecks: if n.rechecks < 255 { (n.rechecks + 1) as u8 } else { 255u8 }, ..n };
    retirement_retry::recheck_distance(m, Phase::Checking, m.rechecks <= FREE_RECHECKS, id);
    reveal(distance);
}
}
