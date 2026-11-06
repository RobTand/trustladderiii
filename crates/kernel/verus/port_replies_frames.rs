//! #806: small local consistency and shared-rule frames for worker replies.
use super::port_replies_contracts::*;
use super::*;

verus! {
/// #806 original slice-one domain: no prerequisite can withhold ingest credit.
pub(super) proof fn empty_prerequisites(w: World, i: int)
    requires 0 <= i < w.nodes@.len(),
    ensures w.dependencies@.len() == 0 ==> dependencies::prerequisites_delivered(w, NodeId(i as u64)),
{
    dependencies::prerequisite_definition(w, NodeId(i as u64));
    dependencies::delivered_definition(w.dependencies@, NodeId(i as u64));
}

/// #806: the phase and schema checks for the eight W1 payloads.
pub(super) open spec fn admitted(w: World, n: Node, reply: Reply) -> bool {
    match (n.phase, reply) {
        (Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }, Reply::ProtectedPaths) => true,
        (Phase::Ingesting, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)) => true,
        (Phase::Auditing, Reply::AuditAgrees | Reply::AuditDisagrees) => true,
        (Phase::Planning { .. }, Reply::PlanLeaf { footprint, .. }) => footprint::valid_footprint(w, footprint),
        (Phase::Planning { .. }, Reply::PlanSplit | Reply::PlanCantMeet) => true,
        _ => false,
    }
}

/// #806: node rules depend only on these world fields; new questions get fresh ids.
pub(super) open spec fn local_context(w: World, a: World, n: Node, reply: Reply) -> bool {
    retirement::node_rules(w, n) && node_headroom(n) && !n.credit.withheld
        && current_item(n) == Some(Item::Await) && admitted(w, n, reply)
        && w.next_question < u64::MAX && a.gates@ == w.gates@
        && a.file_sets@ == w.file_sets@ && a.next_ticket == w.next_ticket
        && a.next_question == (w.next_question + if asks(n, w.fail_fast, reply) { 1u64 } else { 0u64 }) as u64
}

/// #806 shared check_nodes and Q54 assertions for a completed worker reply.
pub(super) open spec fn node_change(b: Node, n: Node, fast: bool, reply: Reply) -> bool {
    n.proposed == b.proposed && (b.adopted ==> n.adopted) && n.next_seq >= b.next_seq
        && (b.phase is Closed ==> n.phase == b.phase)
        && n.credit.earned == b.credit.earned && n.credit.merge_credited == b.credit.merge_credited
        && (fast ==> n.attempt <= if b.attempt > 1 { b.attempt } else { 1u8 })
        && (fast ==> b.replanned || !n.replanned)
        && (fast && n.phase is Planning ==> b.phase is Planning || b.phase == Phase::AwaitingRob)
        && (fast && n.question.is_some() && n.question.unwrap().kind == QKind::CantMeet
            && n.question != b.question ==> reply is PlanLeaf)
        && (n.review_stop != b.review_stop ==> b.review_stop.is_none()
            && n.phase == Phase::Stopping(Outcome::Stopped) && n.credit == b.credit && n.question.is_none())
        && (!fast ==> n.review_stop == b.review_stop)
}

/// #806: complete local checker and resource frame used by the world lift.
pub(super) open spec fn local_result(w: World, a: World, n: Node, reply: Reply) -> bool {
    let next = transitioned(ended(n), w.next_question, w.fail_fast, reply);
    retirement::node_rules(a, next) && node_change(n, next, w.fail_fast, reply)
        && !next.holds && !next.pool && next.ticket.is_none() && !next.credit.withheld
        && next.repo == n.repo
}

/// #806: increasing next_question preserves every existing node's checker rules.
pub(super) proof fn question_frame(before: World, after: World, n: Node)
    requires before.gates@ == after.gates@, before.file_sets@ == after.file_sets@,
        before.next_ticket == after.next_ticket, before.next_question <= after.next_question,
        retirement::node_rules(before, n),
    ensures retirement::node_rules(after, n),
{
    reveal(retirement::node_rules);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}

/// #806: a selected-node reply that acquires no resource preserves global
/// gates, file exclusion, pool capacity, and unique tickets in the original domain.
pub(super) proof fn world_frame(before: World, after: World)
    requires retirement::domain(before), retirement::accepted(before),
        after.nodes@.len() == 2, after.nodes@[1] == before.nodes@[1],
        retirement::node_rules(after, after.nodes@[0]),
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket, after.next_question >= before.next_question,
        after.pool_capacity == before.pool_capacity, after.dependencies@ == before.dependencies@,
        after.nodes@[0].repo == before.nodes@[0].repo,
        !after.nodes@[0].credit.withheld,
        (after.nodes@[0].holds ==> before.nodes@[0].holds
            && after.nodes@[0].footprint == before.nodes@[0].footprint),
        (after.nodes@[0].pool ==> before.nodes@[0].pool),
        (after.nodes@[0].ticket.is_some() ==> after.nodes@[0].ticket == before.nodes@[0].ticket),
    ensures retirement::domain(after), retirement::accepted(after),
{
    reveal(retirement::accepted);
    question_frame(before, after, after.nodes@[1]);
    assert forall|r: int| 0 <= r < 2 implies
        retirement::holders(after, RepoId(r as u64), 2) <= after.gates@[r].w by {
        assert(retirement::holders(after, RepoId(r as u64), 2)
            <= retirement::holders(before, RepoId(r as u64), 2));
    }
    footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[1].footprint);
}

/// Stopping reply row / #806: a late completion releases only its matching run
/// and job; teardown phase, credit, sequence, question and file locks are framed.
pub(super) open spec fn stopping(n: Node, generation: u64) -> Node {
    Node { run: if n.run == Some(generation) { None } else { n.run },
        pool: n.run != Some(generation) && n.pool,
        job: if n.job == Some(generation) { None } else { n.job }, ..n }
}

/// Stopping reply row / #806: retiring an already-ended operation preserves the
/// complete checker and every shared node/Q54 assertion for arbitrary generation.
pub(super) proof fn stopping_node(w: World, n: Node, generation: u64, reply: Reply)
    requires retirement::node_rules(w, n), n.phase is Stopping,
    ensures retirement::node_rules(w, stopping(n, generation)),
        node_change(n, stopping(n, generation), w.fail_fast, reply),
        stopping(n, generation).repo == n.repo,
        stopping(n, generation).footprint == n.footprint,
        stopping(n, generation).credit == n.credit,
        stopping(n, generation).holds == n.holds,
        stopping(n, generation).pool ==> n.pool,
        stopping(n, generation).ticket == n.ticket,
{
    reveal(retirement::node_rules);
}
}

verus! {
/// #806 ingest_done: exact fields and unchanged remainder preserve the complete node
/// checker and every shared check_nodes/Q54 assertion, with no bounded unrolling.
pub(super) proof fn ingest_done(w: World, a: World, n: Node, size: u8)
    requires local_context(w, a, n, Reply::IngestDone { size }),
    ensures local_result(w, a, n, Reply::IngestDone { size }),
{
    reveal(retirement::node_rules); reveal(retirement::stopped);
    reveal(transitioned); reveal(plan_retry_node);
    footprint::storage_frame(w, a, n.footprint, n.footprint);
    reveal(ingested);
}
}

verus! {
/// #806 ingest_early_end: exact fields and unchanged remainder preserve the complete node
/// checker and every shared check_nodes/Q54 assertion, with no bounded unrolling.
pub(super) proof fn ingest_early_end(w: World, a: World, n: Node, ending: EarlyEnd)
    requires local_context(w, a, n, Reply::IngestEarlyEnd(ending)),
    ensures local_result(w, a, n, Reply::IngestEarlyEnd(ending)),
{
    reveal(retirement::node_rules); reveal(retirement::stopped);
    reveal(transitioned); reveal(plan_retry_node);
    footprint::storage_frame(w, a, n.footprint, n.footprint);
    reveal(ingested);
}
}

verus! {
/// #806 protected_paths: exact fields and unchanged remainder preserve the complete node
/// checker and every shared check_nodes/Q54 assertion, with no bounded unrolling.
pub(super) proof fn protected_paths(w: World, a: World, n: Node)
    requires local_context(w, a, n, Reply::ProtectedPaths),
    ensures local_result(w, a, n, Reply::ProtectedPaths),
{
    reveal(retirement::node_rules); reveal(retirement::stopped);
    reveal(transitioned); reveal(plan_retry_node);
    footprint::storage_frame(w, a, n.footprint, n.footprint);
}
}

verus! {
/// #806 audit_agrees: exact fields and unchanged remainder preserve the complete node
/// checker and every shared check_nodes/Q54 assertion, with no bounded unrolling.
pub(super) proof fn audit_agrees(w: World, a: World, n: Node)
    requires local_context(w, a, n, Reply::AuditAgrees),
    ensures local_result(w, a, n, Reply::AuditAgrees),
{
    reveal(retirement::node_rules); reveal(retirement::stopped);
    reveal(transitioned); reveal(plan_retry_node);
    footprint::storage_frame(w, a, n.footprint, n.footprint);
}
}

verus! {
/// #806 audit_disagrees: exact fields and unchanged remainder preserve the complete node
/// checker and every shared check_nodes/Q54 assertion, with no bounded unrolling.
pub(super) proof fn audit_disagrees(w: World, a: World, n: Node)
    requires local_context(w, a, n, Reply::AuditDisagrees),
    ensures local_result(w, a, n, Reply::AuditDisagrees),
{
    reveal(retirement::node_rules); reveal(retirement::stopped);
    reveal(transitioned); reveal(plan_retry_node);
    footprint::storage_frame(w, a, n.footprint, n.footprint);
}
}

verus! {
/// #806 plan_leaf: exact fields and unchanged remainder preserve the complete node
/// checker and every shared check_nodes/Q54 assertion, with no bounded unrolling.
pub(super) proof fn plan_leaf(w: World, a: World, n: Node, footprint: Footprint, fits: bool)
    requires local_context(w, a, n, Reply::PlanLeaf { footprint, fits }),
    ensures local_result(w, a, n, Reply::PlanLeaf { footprint, fits }),
{
    reveal(retirement::node_rules); reveal(retirement::stopped);
    reveal(transitioned); reveal(plan_retry_node);
    footprint::storage_frame(w, a, n.footprint, n.footprint);
    footprint::storage_frame(w, a, footprint, footprint);
}
}

verus! {
/// #806 plan_split: exact fields and unchanged remainder preserve the complete node
/// checker and every shared check_nodes/Q54 assertion, with no bounded unrolling.
pub(super) proof fn plan_split(w: World, a: World, n: Node)
    requires local_context(w, a, n, Reply::PlanSplit),
    ensures local_result(w, a, n, Reply::PlanSplit),
{
    reveal(retirement::node_rules); reveal(retirement::stopped);
    reveal(transitioned); reveal(plan_retry_node);
    footprint::storage_frame(w, a, n.footprint, n.footprint);
}
}

verus! {
/// #806 plan_cant_meet: exact fields and unchanged remainder preserve the complete node
/// checker and every shared check_nodes/Q54 assertion, with no bounded unrolling.
pub(super) proof fn plan_cant_meet(w: World, a: World, n: Node)
    requires local_context(w, a, n, Reply::PlanCantMeet),
    ensures local_result(w, a, n, Reply::PlanCantMeet),
{
    reveal(retirement::node_rules); reveal(retirement::stopped);
    reveal(transitioned); reveal(plan_retry_node);
    footprint::storage_frame(w, a, n.footprint, n.footprint);
}
}
