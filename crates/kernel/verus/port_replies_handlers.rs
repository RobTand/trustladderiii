//! #806: small node-composition lemmas for the real planner retry branches.
use super::port_replies_contracts::*;
use super::*;

verus! {
/// Planner retry row / #806: reconstruct the exact node from its normalized fields.
pub(super) proof fn plan_exact(n: Node, after: Node, id: u64, fast: bool, cant: bool)
    requires n.phase is Planning, plan_post(n, after, id, fast, cant),
    ensures after == plan_retry_node(n, id, fast, cant),
{ reveal(plan_post); reveal(plan_retry_node); reveal(retirement::stopped); }

/// Q54 / #806: compose the real stop helper with the preceding retry-count update.
pub(super) proof fn plan_stop_result(n: Node, mid: Node, after: Node, id: u64, fast: bool, cant: bool)
    requires fast && cant,
        mid == (Node { plan_retries: if n.plan_retries < u8::MAX { (n.plan_retries + 1) as u8 }
            else { u8::MAX }, ..n }),
        after == retirement::stopped(Node { review_stop: Some(ReviewStop::PlanCantMeet), ..mid }, Outcome::Stopped),
    ensures plan_post(n, after, id, fast, cant),
{ reveal(plan_post); reveal(plan_retry_node); }

/// Planner retry row / #806: compose the real phase reset with the first retry count.
pub(super) proof fn plan_again_result(n: Node, mid: Node, after: Node, id: u64, fast: bool, cant: bool)
    requires !(fast && cant), n.plan_retries == 0,
        mid == (Node { plan_retries: 1, ..n }), after == retirement::phased(mid, n.phase),
    ensures plan_post(n, after, id, fast, cant),
{ reveal(plan_post); reveal(plan_retry_node); }

/// Planner exhaustion row / #806: compose the real Ask update with the retry-count reset.
pub(super) proof fn plan_ask_result(n: Node, mid: Node, after: Node, id: u64, fast: bool, cant: bool)
    requires !(fast && cant), n.plan_retries >= 1,
        mid == (Node { plan_retries: 0, ..n }),
        after == asked(mid, id, QKind::CantMeet,
            Cont::Planning { must_split: matches!(n.phase, Phase::Planning { must_split: true }) }, 1),
    ensures plan_post(n, after, id, fast, cant),
{ reveal(plan_post); reveal(plan_retry_node); }

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

}
