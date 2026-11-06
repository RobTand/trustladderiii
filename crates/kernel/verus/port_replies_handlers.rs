//! #806: small field and node composition lemmas for real worker-reply branches.
use super::port_replies_contracts::*;
use super::*;

verus! {
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

/// #806 ingest/audit row: compose this real handler branch into normalized fields.
pub(super) proof fn fields_done(n: Node, after: Node, id: u64, size: u8)
    requires n.generation < u64::MAX, n.phase == Phase::Ingesting, after == ingested(n, size, None),
    ensures ingest_post(n, after, id, Reply::IngestDone { size }),
{ reveal(ingest_post); reveal(ingest_stable); reveal(ingest_changes); reveal(ingested); }

/// #806 ingest/audit row: compose this real handler branch into normalized fields.
pub(super) proof fn fields_early(n: Node, after: Node, id: u64, e: EarlyEnd)
    requires n.generation < u64::MAX, n.phase == Phase::Ingesting, after == ingested(n, 1, Some(e)),
    ensures ingest_post(n, after, id, Reply::IngestEarlyEnd(e)),
{ reveal(ingest_post); reveal(ingest_stable); reveal(ingest_changes); reveal(ingested); }

/// #806 ingest/audit row: compose this real handler branch into normalized fields.
pub(super) proof fn fields_agreed_stop(n: Node, after: Node, id: u64, e: EarlyEnd)
    requires n.generation < u64::MAX, n.phase == Phase::Auditing, n.early_end == Some(e), after == retirement::stopped(n, early_outcome(e)),
    ensures ingest_post(n, after, id, Reply::AuditAgrees),
{ reveal(ingest_post); reveal(ingest_stable); reveal(ingest_changes); reveal(retirement::stopped); }

/// #806 ingest/audit row: compose this real handler branch into normalized fields.
pub(super) proof fn fields_agreed_open(n: Node, after: Node, id: u64)
    requires n.generation < u64::MAX, n.phase == Phase::Auditing, n.early_end.is_none(), after == retirement::phased(n, Phase::Unplanned),
    ensures ingest_post(n, after, id, Reply::AuditAgrees),
{ reveal(ingest_post); reveal(ingest_stable); reveal(ingest_changes);  }

/// #806 ingest/audit row: compose this real handler branch into normalized fields.
pub(super) proof fn fields_audit_first(n: Node, after: Node, id: u64, mid: Node)
    requires n.generation < u64::MAX, n.phase == Phase::Auditing, n.audit_rounds == 0, mid == (Node { audit_rounds: 1, ..n }), after == retirement::phased(mid, Phase::Ingesting),
    ensures ingest_post(n, after, id, Reply::AuditDisagrees),
{ reveal(ingest_post); reveal(ingest_stable); reveal(ingest_changes);  }

/// #806 ingest/audit row: compose this real handler branch into normalized fields.
pub(super) proof fn fields_audit_ask(n: Node, after: Node, id: u64, mid: Node)
    requires n.generation < u64::MAX, n.phase == Phase::Auditing, n.audit_rounds >= 1, mid == (Node { audit_rounds: if n.audit_rounds < u8::MAX { (n.audit_rounds + 1) as u8 } else { u8::MAX }, ..n }), after == asked(mid, id, QKind::AuditDisagreement, match n.early_end { Some(e) => Cont::Stop(early_outcome(e)), None => Cont::Unplanned }, 0),
    ensures ingest_post(n, after, id, Reply::AuditDisagrees),
{ reveal(ingest_post); reveal(ingest_stable); reveal(ingest_changes);  }

/// #806 ingest/audit row: compose this real handler branch into normalized fields.
pub(super) proof fn fields_protected(n: Node, after: Node, id: u64)
    requires n.generation < u64::MAX, ingest_accepted(n, Reply::ProtectedPaths), after == retirement::stopped(n, Outcome::NotActionable),
    ensures ingest_post(n, after, id, Reply::ProtectedPaths),
{ reveal(ingest_post); reveal(ingest_stable); reveal(ingest_changes); reveal(retirement::stopped); }

/// Ingest/audit row / #806: reconstruct this reply's exact node from its fields.
pub(super) proof fn exact_ingest_done(n: Node, after: Node, id: u64, fast: bool, reply: Reply)
    requires matches!(reply, Reply::IngestDone { .. }), ingest_accepted(n, reply), ingest_post(n, after, id, reply),
    ensures node_post(n, after, id, fast, reply),
{ reveal(ingest_post); reveal(ingest_stable); reveal(ingest_changes); reveal(node_post); reveal(transitioned); reveal(ingested); reveal(retirement::stopped); }

/// Ingest/audit row / #806: reconstruct this reply's exact node from its fields.
pub(super) proof fn exact_ingest_early(n: Node, after: Node, id: u64, fast: bool, reply: Reply)
    requires matches!(reply, Reply::IngestEarlyEnd(_)), ingest_accepted(n, reply), ingest_post(n, after, id, reply),
    ensures node_post(n, after, id, fast, reply),
{ reveal(ingest_post); reveal(ingest_stable); reveal(ingest_changes); reveal(node_post); reveal(transitioned); reveal(ingested); reveal(retirement::stopped); }

/// Ingest/audit row / #806: reconstruct this reply's exact node from its fields.
pub(super) proof fn exact_audit_agrees(n: Node, after: Node, id: u64, fast: bool, reply: Reply)
    requires matches!(reply, Reply::AuditAgrees), ingest_accepted(n, reply), ingest_post(n, after, id, reply),
    ensures node_post(n, after, id, fast, reply),
{ reveal(ingest_post); reveal(ingest_stable); reveal(ingest_changes); reveal(node_post); reveal(transitioned); reveal(ingested); reveal(retirement::stopped); }

/// Ingest/audit row / #806: reconstruct this reply's exact node from its fields.
pub(super) proof fn exact_audit_disagrees(n: Node, after: Node, id: u64, fast: bool, reply: Reply)
    requires matches!(reply, Reply::AuditDisagrees), ingest_accepted(n, reply), ingest_post(n, after, id, reply),
    ensures node_post(n, after, id, fast, reply),
{
    if n.audit_rounds == 0 {
        assert(node_post(n, after, id, fast, reply)) by {
            reveal(ingest_post); reveal(ingest_stable); reveal(ingest_changes); reveal(node_post); reveal(transitioned);
        }
    } else {
        assert(node_post(n, after, id, fast, reply)) by {
            reveal(ingest_post); reveal(ingest_stable); reveal(ingest_changes); reveal(node_post); reveal(transitioned);
        }
    }
}

/// Ingest/audit row / #806: reconstruct this reply's exact node from its fields.
pub(super) proof fn exact_protected(n: Node, after: Node, id: u64, fast: bool, reply: Reply)
    requires matches!(reply, Reply::ProtectedPaths), ingest_accepted(n, reply), ingest_post(n, after, id, reply),
    ensures node_post(n, after, id, fast, reply),
{ reveal(ingest_post); reveal(ingest_stable); reveal(ingest_changes); reveal(node_post); reveal(transitioned); reveal(ingested); reveal(retirement::stopped); }

/// Ingest/audit rows / #806: select a separately verified reconstruction lemma.
pub(super) proof fn ingest_exact(n: Node, after: Node, id: u64, fast: bool, reply: Reply)
    requires ingest_accepted(n, reply), ingest_post(n, after, id, reply),
    ensures node_post(n, after, id, fast, reply),
{
    match reply {
        Reply::IngestDone { .. } => exact_ingest_done(n, after, id, fast, reply),
        Reply::IngestEarlyEnd(_) => exact_ingest_early(n, after, id, fast, reply),
        Reply::AuditAgrees => exact_audit_agrees(n, after, id, fast, reply),
        Reply::AuditDisagrees => exact_audit_disagrees(n, after, id, fast, reply),
        Reply::ProtectedPaths => exact_protected(n, after, id, fast, reply),
        _ => {},
    }
}

/// #771 audit row: the successful agreement reduces distance on its exact node frame.
pub(super) proof fn audit_progress(n: Node, after: Node, id: u64, fast: bool)
    requires successful_reply_input(n, Reply::AuditAgrees),
        ingest_post(n, after, id, Reply::AuditAgrees),
    ensures distance(after) < distance(n),
{
    reveal(ingest_post); reveal(ingest_stable); reveal(ingest_changes);
    reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank);
}

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

/// #806 planner frame: keep stable and branch-local field queries separate.
pub(super) proof fn plan_join(n: Node, after: Node, id: u64, fast: bool, cant: bool)
    requires plan_stable(n, after), plan_changes(n, after, id, fast, cant),
    ensures plan_post(n, after, id, fast, cant),
{ reveal(plan_post); }

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

/// Planner retry row / #806: reconstruct the exact node from its normalized fields.
pub(super) proof fn plan_exact(n: Node, after: Node, id: u64, fast: bool, cant: bool)
    requires n.phase is Planning, plan_post(n, after, id, fast, cant),
    ensures after == plan_retry_node(n, id, fast, cant),
{ reveal(plan_post); reveal(plan_stable); reveal(plan_changes); reveal(plan_retry_node); reveal(retirement::stopped); }

/// #806 reply frame: carry a zero-output world frame to the real output lengths.
pub(super) proof fn reply_output_bound(before: World, after: World, i: int,
    old_len: usize, new_len: usize)
    requires reply_bounds(before, after, i, 0, 0), new_len as int <= old_len as int + 1,
    ensures reply_bounds(before, after, i, old_len, new_len),
{ reveal(reply_bounds); }

/// #806 audit frame: join the separately scoped stable and changed field queries.
pub(super) proof fn ingest_join(n: Node, after: Node, id: u64, reply: Reply)
    requires ingest_stable(n, after), ingest_changes(n, after, id, reply),
    ensures ingest_post(n, after, id, reply),
{ reveal(ingest_post); }

}
