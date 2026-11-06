//! #860: small forwarding lemmas keep exact frames out of unrelated solver queries.
use super::port_t860_contracts::*;
use super::*;

verus! {
/// Auditing / #860: expose the exact audit result only inside its own small query.
pub(super) proof fn audit_finished(b: Node, a: Node)
    requires a == {
        let updated = Node { audit_rounds: if b.audit_rounds < 255 { (b.audit_rounds + 1) as u8 } else { 255u8 }, ..b };
        if b.audit_rounds == 0 { retirement::phased(updated, Phase::Ingesting) }
        else { retirement::stopped(Node { review_stop: Some(ReviewStop::AuditDisagreement), ..updated }, Outcome::Stopped) }
    },
    ensures a == audit_node(b),
        successful_reply_input(b, Reply::AuditDisagrees) ==> distance(a) < distance(b),
        a.credit == b.credit,
        (b.phase == Phase::Auditing && b.audit_rounds >= 1 ==> a.phase == Phase::Stopping(Outcome::Stopped)
            && a.question.is_none() && a.review_stop == Some(ReviewStop::AuditDisagreement)),
{ reveal(audit_node); reveal(retirement::stopped); reveal(distance);
    reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); }

/// Stale/stopping reply rows / #860: compose the actual branch's small node update.
pub(super) proof fn inactive(b: World, a: World, i: int, g: u64, reply: Reply, bo: Outbox, ao: Outbox)
    requires 0 <= i < b.nodes@.len(), a.next_question == b.next_question, a.file_sets@ == b.file_sets@, ao == bo,
        a.nodes@.len() == b.nodes@.len(),
        forall|j: int| 0 <= j < b.nodes@.len() && j != i ==> a.nodes@[j] == b.nodes@[j],
        a.gates@ == b.gates@, a.dependencies@ == b.dependencies@,
        a.pool_capacity == b.pool_capacity, a.fail_fast == b.fail_fast, a.next_ticket == b.next_ticket,
        reply == Reply::AuditDisagrees ==> (b.nodes@[i].phase is Stopping
            && stopping_post(b.nodes@[i], a.nodes@[i], g)
            || !(b.nodes@[i].phase is Stopping) && !active(b.nodes@[i], g) && a.nodes@[i] == b.nodes@[i]),
    ensures reply_post(b, a, i, g, reply, bo, ao),
{
    assert(a.nodes@ =~= b.nodes@.update(i, a.nodes@[i]));
    reveal(reply_post); reveal(reply_node); reveal(failed_post); reveal(stopping_post);
}

/// Q50 / #860: a rejected audit payload composes the actual failed-run contract.
pub(super) proof fn rejected(b: World, a: World, i: int, g: u64, reply: Reply, bo: Outbox, ao: Outbox)
    requires 0 <= i < b.nodes@.len(), a.next_question == b.next_question, a.file_sets@ == b.file_sets@,
        a.nodes@.len() == b.nodes@.len(),
        forall|j: int| 0 <= j < b.nodes@.len() && j != i ==> a.nodes@[j] == b.nodes@[j],
        a.gates@ == b.gates@, a.dependencies@ == b.dependencies@,
        a.pool_capacity == b.pool_capacity, a.fail_fast == b.fail_fast, a.next_ticket == b.next_ticket,
        reply == Reply::AuditDisagrees ==> !(b.nodes@[i].phase is Stopping)
            && active(b.nodes@[i], g) && b.nodes@[i].phase != Phase::Auditing
            && failed_post(b.nodes@[i], a.nodes@[i], bo, ao),
    ensures reply_post(b, a, i, g, reply, bo, ao),
{
    assert(a.nodes@ =~= b.nodes@.update(i, a.nodes@[i]));
    reveal(reply_post); reveal(reply_node); reveal(failed_post); reveal(stopping_post);
}

/// Auditing / #860: compose actual end_operation and reply_ingest frames.
pub(super) proof fn admitted(b: World, ended_w: World, a: World, i: int, g: u64, reply: Reply,
    bo: Outbox, ao: Outbox)
    requires 0 <= i < b.nodes@.len(), 0 <= i < ended_w.nodes@.len(),
        ended_w.next_question == b.next_question, ended_w.file_sets@ == b.file_sets@,
        transition_post(ended_w, a, i, reply, bo, ao),
        reply_bounds(ended_w, a, i, bo.len, ao.len),
        ended_w.nodes@.len() == b.nodes@.len(),
        forall|j: int| 0 <= j < b.nodes@.len() && j != i ==> ended_w.nodes@[j] == b.nodes@[j],
        ended_w.gates@ == b.gates@, ended_w.dependencies@ == b.dependencies@,
        ended_w.pool_capacity == b.pool_capacity, ended_w.fail_fast == b.fail_fast, ended_w.next_ticket == b.next_ticket,
        reply == Reply::AuditDisagrees ==> b.nodes@[i].phase == Phase::Auditing
            && active(b.nodes@[i], g) && ended_post(b.nodes@[i], ended_w.nodes@[i]),
    ensures reply_post(b, a, i, g, reply, bo, ao),
{
    reveal(reply_bounds);
    assert(a.nodes@ =~= b.nodes@.update(i, a.nodes@[i]));
    reveal(reply_post); reveal(reply_node); reveal(transition_post); reveal(ended_post); reveal(ended); reveal(reply_bounds);
}

/// #860: unrelated payloads make no audit claim.
pub(super) proof fn irrelevant(b: World, a: World, i: int, g: u64, reply: Reply, bo: Outbox, ao: Outbox)
    requires reply != Reply::AuditDisagrees,
    ensures reply_post(b, a, i, g, reply, bo, ao),
{ reveal(reply_post); }

}
