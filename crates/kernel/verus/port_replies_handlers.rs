//! #806: small node-composition lemmas for the real planner retry branches.
use super::port_replies_contracts::*;
use super::*;

verus! {
/// Planner retry row / #806: reconstruct the exact node from its normalized fields.
pub(super) proof fn plan_exact(n: Node, after: Node, id: u64, fast: bool, cant: bool)
    requires n.phase is Planning, plan_post(n, after, id, fast, cant),
    ensures after == plan_retry_node(n, id, fast, cant),
{ reveal(plan_post); reveal(plan_stable); reveal(plan_changes); reveal(plan_retry_node); reveal(retirement::stopped); }

/// Ingest completion row / #806: forward the real finish_ingest node frame.
pub(super) proof fn ingest_done(n: Node, after: Node, id: u64, fast: bool, size: u8)
    requires after == ingested(n, size, None),
    ensures node_post(n, after, id, fast, Reply::IngestDone { size }),
{ reveal(node_post); reveal(transitioned); }

/// Early ingest row / #806: retain the worker's exact early ending.
pub(super) proof fn ingest_early(n: Node, after: Node, id: u64, fast: bool, e: EarlyEnd)
    requires after == ingested(n, 1, Some(e)),
    ensures node_post(n, after, id, fast, Reply::IngestEarlyEnd(e)),
{ reveal(node_post); reveal(transitioned); }

/// Audit agreement row / #806: forward either real stopping or Unplanned phase reset.
pub(super) proof fn audit_agrees(n: Node, after: Node, id: u64, fast: bool)
    requires after == match n.early_end {
        Some(e) => retirement::stopped(n, early_outcome(e)),
        None => retirement::phased(n, Phase::Unplanned),
    },
    ensures node_post(n, after, id, fast, Reply::AuditAgrees),
{ reveal(node_post); reveal(transitioned); }

/// Audit objection row / #806: compose the saturating counter with the real handler.
pub(super) proof fn audit_disagrees(n: Node, mid: Node, after: Node, id: u64, fast: bool)
    requires mid == (Node { audit_rounds: if n.audit_rounds < u8::MAX {
        (n.audit_rounds + 1) as u8 } else { u8::MAX }, ..n }),
        after == if mid.audit_rounds < 2 { retirement::phased(mid, Phase::Ingesting) }
        else { asked(mid, id, QKind::AuditDisagreement,
            match n.early_end { Some(e) => Cont::Stop(early_outcome(e)), None => Cont::Unplanned }, 0) },
    ensures node_post(n, after, id, fast, Reply::AuditDisagrees),
{ reveal(node_post); reveal(transitioned); }

/// Protected paths row / #806: forward the exact real stopping update.
pub(super) proof fn protected(n: Node, after: Node, id: u64, fast: bool)
    requires after == retirement::stopped(n, Outcome::NotActionable),
    ensures node_post(n, after, id, fast, Reply::ProtectedPaths),
{ reveal(node_post); reveal(transitioned); }

/// #806 planning leaf row: forward only the real successful leaf's primitive frame.
pub(super) proof fn leaf_ready_result(n: Node, after: Node, id: u64, fast: bool, footprint: Footprint, fits: bool)
    requires fits, !footprint::footprint_empty(footprint),
        n.phase == (Phase::Planning { must_split: false }),
        after == retirement::phased(Node { footprint, attempt: 1, review_restarted: false,
            round: 0, plan_retries: 0, ..n }, Phase::Ready(Resume::Making)),
    ensures node_post(n, after, id, fast, Reply::PlanLeaf { footprint, fits }),
{ reveal(node_post); reveal(transitioned); }

/// #806 planning retry row: forward the real normalized retry after a non-fitting leaf.
pub(super) proof fn leaf_retry_result(n: Node, after: Node, id: u64, fast: bool, footprint: Footprint, fits: bool)
    requires !fits || footprint::footprint_empty(footprint) || n.phase == (Phase::Planning { must_split: true }),
        after == plan_retry_node(n, id, fast, false),
    ensures node_post(n, after, id, fast, Reply::PlanLeaf { footprint, fits }),
{ reveal(node_post); reveal(transitioned); }

/// #806 split row: forward the real Ask frame without opening other reply cases.
pub(super) proof fn split_result(n: Node, after: Node, id: u64, fast: bool)
    requires after == asked(n, id, QKind::NeedsSplitting, Cont::Nowhere, 0),
    ensures node_post(n, after, id, fast, Reply::PlanSplit),
{ reveal(node_post); reveal(transitioned); }

/// Q54 / #806: forward the real planner can't-meet retry frame.
pub(super) proof fn cant_result(n: Node, after: Node, id: u64, fast: bool)
    requires after == plan_retry_node(n, id, fast, true),
    ensures node_post(n, after, id, fast, Reply::PlanCantMeet),
{ reveal(node_post); reveal(transitioned); }

/// #771 planning rows: the successful leaf or split reduces distance on its exact frame.
pub(super) proof fn plan_progress(n: Node, after: Node, id: u64, fast: bool, reply: Reply)
    requires matches!(reply, Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet),
        successful_reply_input(n, reply), node_post(n, after, id, fast, reply),
        !footprint::footprint_empty(footprint::inline_footprint(1)),
    ensures distance(after) < distance(n),
{
    reveal(node_post); reveal(transitioned); reveal(plan_retry_node);
    reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank);
}

/// #806 output rows: an exact notice append preserves every output rule.
pub(super) proof fn output_notices(before: Outbox, after: Outbox, i: int, emitted: Option<Notice>)
    requires output_post(before, after, i, emitted), retirement::notices_only(before),
    ensures retirement::notices_only(after),
{
    reveal(output_post); reveal(retirement::notices_only);
    assert forall|k: int| 0 <= k < OUTBOX implies match #[trigger] after.items@[k] {
        None | Some(Out::Notice { .. }) => true, _ => false,
    } by {
        if let Some(_) = emitted {
            if k != before.len { assert(after.items@[k] == before.items@[k]); }
        }
    }
}

/// Q54 / #806: forward exact worker planning fields to W5's shared result relation.
pub(super) proof fn plan_forward(n: Node, after: Node, id: u64, next: u64,
    fast: bool, cant: bool, before_out: Outbox, after_out: Outbox, i: int)
    requires n.phase is Planning, plan_post(n, after, id, fast, cant),
        next == (id + if !(fast && cant) && n.plan_retries >= 1 { 1u64 } else { 0u64 }) as u64,
        id < u64::MAX,
        output_post(before_out, after_out, i,
            if !(fast && cant) && n.plan_retries >= 1 { Some(Notice::Ask { question: id }) } else { None }),
    ensures port_failfast_diversion::plan_result(n, after, fast, cant, id, next, before_out, after_out),
{
    plan_exact(n, after, id, fast, cant);
    reveal(port_failfast_diversion::plan_result); reveal(port_failfast_diversion::plan_fields);
    reveal(port_failfast_diversion::plan_count); reveal(plan_retry_node); reveal(output_post);
}

/// #806 planner frame: keep stable and branch-local field queries separate.
pub(super) proof fn plan_join(n: Node, after: Node, id: u64, fast: bool, cant: bool)
    requires plan_stable(n, after), plan_changes(n, after, id, fast, cant),
    ensures plan_post(n, after, id, fast, cant),
{ reveal(plan_post); }

}
