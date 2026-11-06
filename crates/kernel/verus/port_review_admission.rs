//! #806: compose exact rebase frames outside the real handler query.
use super::port_review_reply::*;
use super::*;

verus! {
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
