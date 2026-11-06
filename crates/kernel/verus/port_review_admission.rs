//! #806: compose the real admission branches without unfolding them in on_reply.
use super::port_review_reply::*;
use super::*;

verus! {
/// State-table reply rows: all admission branches share the existing outer frame.
pub(super) open spec fn outer(before: World, after: World, i: int) -> bool {
    after.nodes@.len() == before.nodes@.len()
        && (forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> after.nodes@[j] == before.nodes@[j])
        && after.gates@ == before.gates@ && after.next_ticket == before.next_ticket
        && after.pool_capacity == before.pool_capacity && after.fail_fast == before.fail_fast
        && after.dependencies@ == before.dependencies@
}

/// #806: fold the shared reply frame outside the executable admission query.
pub(super) proof fn mount(before: World, after: World, i: int, generation: u64,
    reply: Reply, out_before: Outbox, out_after: Outbox)
    requires outer(before, after, i),
        reply_result(before, after, i, generation, reply, out_before, out_after),
    ensures reply_post(before, after, i, generation, reply, out_before, out_after),
{ reveal(reply_post); }

/// Q30 stopping and stale rows: release matching ownership or stutter exactly.
pub(super) proof fn inactive(before: World, after: World, i: int, generation: u64,
    reply: Reply, out_before: Outbox, out_after: Outbox)
    requires before.nodes@[i].phase is Stopping || !active(before.nodes@[i], generation),
        after.nodes@[i] == if before.nodes@[i].phase is Stopping {
            stopping_reply(before.nodes@[i], generation) } else { before.nodes@[i] },
        after.next_question == before.next_question, after.file_sets@ == before.file_sets@,
        out_after == out_before,
    ensures reply_result(before, after, i, generation, reply, out_before, out_after),
        (settle_shape(before.nodes@[i]) ==> settle_shape(after.nodes@[i])),
{ reveal(reply_result); }

/// State-table schema-rejected rows: forward the exact real failed-run result.
pub(super) proof fn rejected(before: World, after: World, i: int, generation: u64,
    reply: Reply, out_before: Outbox, out_after: Outbox)
    requires !(before.nodes@[i].phase is Stopping),
        active(before.nodes@[i], generation), !accepted_reply(before, before.nodes@[i], reply),
        after.nodes@[i] == port_review_retry::failed_run(before.nodes@[i]),
        after.next_question == before.next_question, after.file_sets@ == before.file_sets@,
        retirement::notices_only(out_before) ==> retirement::notices_only(out_after),
    ensures reply_result(before, after, i, generation, reply, out_before, out_after),
{ reveal(reply_result); }

/// #806: the shared accepted relation is vacuous outside both selected families.
pub(super) proof fn unselected(before: World, after: World, i: int, source: Node,
    reply: Reply, out_before: Outbox, out_after: Outbox)
    requires !selected(reply), !port_replies_contracts::selected(reply), !port_replies_maker::selected(reply),
    ensures transition_post(before, after, i, source, reply, out_before, out_after),
{ reveal(transition_post); reveal(port_replies_contracts::transition_post); reveal(port_replies_maker::transition_post); }

/// State-table accepted rows: compose end_operation before the selected handler.
pub(super) proof fn accepted(before: World, middle: World, after: World, i: int,
    generation: u64, reply: Reply, out_before: Outbox, out_after: Outbox)
    requires !(before.nodes@[i].phase is Stopping),
        active(before.nodes@[i], generation), accepted_reply(before, before.nodes@[i], reply),
        middle.nodes@[i] == ended(before.nodes@[i]),
        middle.next_question == before.next_question, middle.fail_fast == before.fail_fast,
        middle.file_sets@ == before.file_sets@, middle.dependencies@ == before.dependencies@,
        transition_post(middle, after, i, middle.nodes@[i], reply, out_before, out_after),
    ensures reply_result(before, after, i, generation, reply, out_before, out_after),
{
    reveal(reply_result); reveal(transition_post); reveal(port_replies_contracts::transition_post);
    reveal(accepted_reply);
    match reply {
        Reply::PlanLeaf { footprint, .. } => footprint::storage_frame(before, middle, footprint, footprint),
        Reply::MakerNeedsFiles { files, .. } => footprint::storage_frame(before, middle, files, files),
        _ => {},
    }

    if port_replies_maker::selected(reply) {
        port_replies_maker::transport(before, middle, after, i, ended(before.nodes@[i]),
            reply, out_before, out_after);
    } else {
        assert(port_replies_maker::transition_post(before, after, i, ended(before.nodes@[i]),
            reply, out_before, out_after)) by { reveal(port_replies_maker::transition_post); }
    }

}
/// #805 rebase rows: the exact accepted transition preserves the progress decrease.
pub(super) proof fn rebase_progress(before: World, after: World, i: int,
    reply: Reply, out_before: Outbox, out_after: Outbox)
    requires matches!(reply, Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges | Reply::RunFailed),
        transition_post(before, after, i, before.nodes@[i], reply, out_before, out_after),
    ensures successful_reply_input(before.nodes@[i], reply)
        ==> distance(after.nodes@[i]) < distance(before.nodes@[i]),
{
    reveal(transition_post); reveal(transition); reveal(port_review_retry::rechecked);
    reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank);
}

/// #806 RebaseClean: compose the real observed-head update with the recheck pair.
pub(super) proof fn clean_pair(n: Node, updated: Node, after: Node, head: HeadId,
    fail_fast: bool, id: u64, next: u64)
    requires updated == (Node { head: Some(head), checked: None,
            review_restarted: n.head == Some(head) && n.review_restarted, ..n }),
        port_review_retry::recheck_post(updated, after, Phase::Checking, id, next),
    ensures (after, next) == transition(n, fail_fast, id, Reply::RebaseClean { head }),
{
    reveal(port_review_retry::recheck_post); reveal(transition);
}


}
