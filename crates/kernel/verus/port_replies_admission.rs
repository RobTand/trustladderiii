//! #806: compose exact handler frames through real reply admission.
use super::port_replies_contracts::*;
use super::port_replies_frames::*;
use super::port_replies_safety::*;
use super::*;

verus! {
/// #806: retain check_apply's payload-specific ProtectedPaths and Q54 assertions.
pub(super) open spec fn specific(before: World, after: World, generation: u64, reply: Reply) -> bool {
    let b = before.nodes@[0]; let n = after.nodes@[0];
    (reply is ProtectedPaths && matches!(b.phase, Phase::Ingesting | Phase::Auditing | Phase::Planning { .. })
        && current_item(b) == Some(Item::Await) && b.run == Some(generation)
        ==> n.phase == Phase::Stopping(Outcome::NotActionable) && n.credit == b.credit
            && n.question.is_none() && n.plan_retries == b.plan_retries)
        && (reply is PlanCantMeet && before.fail_fast && b.phase is Planning
            && current_item(b) == Some(Item::Await) && b.run == Some(generation)
            ==> n.phase == Phase::Stopping(Outcome::Stopped)
                && n.review_stop == Some(ReviewStop::PlanCantMeet))
}

/// Q50 / #806: consume the shared exact failed-run contract, rather than
/// duplicating failure dispatch or restricting a rejected payload to its phase.
pub(super) proof fn rejected_node(w: World, n: Node, reply: Reply)
    requires retirement::node_rules(w, n), node_headroom(n),
        current_item(n) == Some(Item::Await), !(n.phase is Stopping),
    ensures retirement::node_rules(w, port_review_retry::failed_run(n)),
        node_change(n, port_review_retry::failed_run(n), w.fail_fast, reply),
        port_review_retry::failed_run(n).credit == n.credit,
        (port_review_retry::failed_run(n).holds ==> n.holds),
        !port_review_retry::failed_run(n).pool,
        port_review_retry::failed_run(n).repo == n.repo,
        port_review_retry::failed_run(n).footprint == n.footprint,
        port_review_retry::failed_run(n).ticket.is_none(),
{
    reveal(retirement::node_rules);
    reveal(port_review_retry::failed_run);
    if n.run_failures == 0 {
        port_replies_failure::rejected_once(w, n, port_review_retry::failed_run(n), reply);
    } else {
        port_replies_failure::rejected_incident(w, n, port_review_retry::failed_run(n), reply);
    }
}

/// #806: local result consumed by the unchanged global resource frame.
#[verifier::opaque]
pub(super) open spec fn node_result(before: World, after: World, reply: Reply) -> bool {
    let b = before.nodes@[0]; let n = after.nodes@[0];
    retirement::node_rules(after, n) && node_change(b, n, before.fail_fast, reply)
        && !n.credit.withheld && n.repo == b.repo
        && (n.holds ==> b.holds && n.footprint == b.footprint)
        && (n.pool ==> b.pool) && (n.ticket.is_some() ==> n.ticket == b.ticket)
}

/// #806: every slice-one reply preserves tables and advances only question IDs.
pub(super) open spec fn table_frame(before: World, after: World) -> bool {
    after.nodes@.len() == 2 && after.nodes@[1] == before.nodes@[1]
        && after.gates@ == before.gates@ && after.file_sets@ == before.file_sets@
        && after.dependencies@ == before.dependencies@ && after.fail_fast == before.fail_fast
        && after.next_ticket == before.next_ticket && after.next_question >= before.next_question
        && after.pool_capacity == before.pool_capacity
}

/// #806: late stopping replies release matching ownership without changing tables.
pub(super) proof fn stopped_reply(before: World, after: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires retirement::domain(before), retirement::accepted(before),
        before.nodes@[0].phase is Stopping,
        retirement::notices_only(out_before),
        port_review_reply::reply_post(before, after, 0, generation, reply, out_before, out_after),
    ensures node_result(before, after, reply), table_frame(before, after),
        retirement::notices_only(out_after),
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result); reveal(retirement::accepted); reveal(node_result);
    stopping_node(before, before.nodes@[0], generation, reply);
    retirement::node_frame(before, after, after.nodes@[0]);
}

/// #806: stale replies preserve the exact node and output for arbitrary generations.
pub(super) proof fn stale_reply(before: World, after: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires retirement::domain(before), retirement::accepted(before),
        !(before.nodes@[0].phase is Stopping), !port_review_reply::active(before.nodes@[0], generation),
        retirement::notices_only(out_before),
        port_review_reply::reply_post(before, after, 0, generation, reply, out_before, out_after),
    ensures node_result(before, after, reply), table_frame(before, after),
        retirement::notices_only(out_after),
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result); reveal(retirement::accepted); reveal(node_result);
    retirement::node_frame(before, after, before.nodes@[0]);
}

/// #806: schema rejection, including invalid PlanLeaf, follows the exact failed-run frame.
pub(super) proof fn rejected_reply(before: World, after: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires retirement::domain(before), retirement::accepted(before), retirement::bounded(before),
        !(before.nodes@[0].phase is Stopping), port_review_reply::active(before.nodes@[0], generation),
        !port_review_reply::accepted_reply(before, before.nodes@[0], reply),
        retirement::notices_only(out_before),
        port_review_reply::reply_post(before, after, 0, generation, reply, out_before, out_after),
    ensures node_result(before, after, reply), table_frame(before, after),
        retirement::notices_only(out_after),
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result); reveal(retirement::accepted);
    reveal(retirement::bounded); reveal(node_result);
    rejected_node(before, before.nodes@[0], reply);
    retirement::node_frame(before, after, after.nodes@[0]);
}

/// #806: lift the eight accepted per-reply frames through the shared transition relation.
pub(super) proof fn accepted_reply(before: World, after: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires retirement::domain(before), retirement::accepted(before), retirement::bounded(before),
        selected(reply), !(before.nodes@[0].phase is Stopping),
        port_review_reply::active(before.nodes@[0], generation),
        port_review_reply::accepted_reply(before, before.nodes@[0], reply),
        retirement::notices_only(out_before),
        port_review_reply::reply_post(before, after, 0, generation, reply, out_before, out_after),
    ensures node_result(before, after, reply), table_frame(before, after),
        retirement::notices_only(out_after),
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result); reveal(port_review_reply::transition_post);
    reveal(port_replies_contracts::transition_post); reveal(node_post);
    reveal(retirement::accepted); reveal(retirement::bounded); reveal(node_result);
    reveal(port_review_reply::accepted_reply);
    admitted_node(before, after, reply);
}

/// Q54 / #806: exact transition fields retain both payload-specific assertions.
pub(super) proof fn specific_reply(before: World, after: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires retirement::domain(before), selected(reply),
        port_review_reply::reply_post(before, after, 0, generation, reply, out_before, out_after),
    ensures specific(before, after, generation, reply),
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result); reveal(port_review_reply::transition_post);
    reveal(port_replies_contracts::transition_post); reveal(node_post);
    reveal(transitioned); reveal(plan_retry_node); reveal(retirement::stopped);
}

/// #806: complete checker, all shared node/output assertions and Q54 for every
/// generation, phase and schema result through the single shared reply relation.
pub(super) proof fn reply_safe(before: World, after: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires retirement::domain(before), retirement::accepted(before), retirement::bounded(before),
        selected(reply), retirement::notices_only(out_before),
        port_review_reply::reply_post(before, after, 0, generation, reply, out_before, out_after),
    ensures retirement::domain(after), retirement::accepted(after), shared(before, after, reply),
        table_frame(before, after),
        retirement::notices_only(out_after), specific(before, after, generation, reply),
{
    let n = before.nodes@[0];
    if n.phase is Stopping {
        stopped_reply(before, after, generation, reply, out_before, out_after);
    } else if !port_review_reply::active(n, generation) {
        stale_reply(before, after, generation, reply, out_before, out_after);
    } else if !port_review_reply::accepted_reply(before, n, reply) {
        rejected_reply(before, after, generation, reply, out_before, out_after);
    } else {
        accepted_reply(before, after, generation, reply, out_before, out_after);
    }
    reveal(node_result);
    world_frame(before, after);
    specific_reply(before, after, generation, reply, out_before, out_after);
    assert forall|i: int| 0 <= i < 2 implies
        node_change(before.nodes@[i], #[trigger] after.nodes@[i], before.fail_fast, reply) by {
        assert(i == 0 || i == 1);
    }
}
}
