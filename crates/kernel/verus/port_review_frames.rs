//! #806: reply-local frames and complete-checker composition.
use super::*;

verus! {
/// State-table question rows: increasing the allocated question bound preserves
/// every existing node rule while all other checker inputs stay unchanged.
pub(super) proof fn question_frame(before: World, after: World, n: Node)
    requires before.gates@ == after.gates@, before.file_sets@ == after.file_sets@,
        before.next_ticket == after.next_ticket, before.next_question <= after.next_question,
    ensures retirement::node_rules(before, n) ==> retirement::node_rules(after, n),
{
    reveal(port_review_reply::ended);
    if before.next_question == after.next_question {
        retirement::node_frame(before, after, n);
    } else {
        reveal(retirement::node_rules);
        footprint::storage_frame(before, after, n.footprint, n.footprint);
    }
}

/// State-table Approved row: end the accepted operation and publish the checked head.
pub(super) proof fn approved_rules(w: World, n: Node, head: HeadId)
    requires retirement::node_rules(w, n), node_headroom(n),
        n.phase == Phase::Reviewing, n.head == Some(head), n.checked == Some(head),
    ensures retirement::node_rules(w, retirement::phased(Node { approved: Some(head),
        ..port_review_reply::ended(n) }, Phase::Publishing)),
{
    reveal(port_review_reply::ended);
    reveal(retirement::node_rules);
}


/// State-table schema failure rows: a failed active operation preserves every node rule.
pub(super) proof fn failed_rules(w: World, n: Node)
    requires retirement::node_rules(w, n), node_headroom(n),
        current_item(n) == Some(Item::Await), !(n.phase is Stopping),
    ensures retirement::node_rules(w, port_review_retry::failed_run(n)),
        !port_review_retry::failed_run(n).holds || n.holds,
        !port_review_retry::failed_run(n).pool || n.pool,
        port_review_retry::failed_run(n).ticket.is_none()
            || port_review_retry::failed_run(n).ticket == n.ticket,
        port_review_retry::failed_run(n).repo == n.repo,
        port_review_retry::failed_run(n).footprint == n.footprint,
        port_review_retry::failed_run(n).credit == n.credit,
        port_review_retry::failed_run(n).proposed == n.proposed,
        port_review_retry::failed_run(n).adopted == n.adopted,
        port_review_retry::failed_run(n).next_seq == n.next_seq,
        port_review_retry::failed_run(n).review_stop == n.review_stop,
        port_review_retry::failed_run(n).attempt == n.attempt,
        port_review_retry::failed_run(n).replanned == n.replanned,
        port_review_retry::failed_run(n).question == n.question,
{
    reveal(port_review_reply::ended);
    reveal(retirement::node_rules);
    reveal(port_review_retry::failed_run);
    reveal(retirement::incident);
    match n.phase {
        Phase::Ingesting => {},
        Phase::Auditing => {},
        Phase::Planning { .. } => {},
        Phase::Making => {},
        Phase::Checking => {},
        Phase::Reviewing => {},
        Phase::Rebasing => {},
        _ => {},
    }
}


/// #806 changes row: preserve every complete-checker node rule.
pub(super) proof fn changes_rules(w: World, n: Node, fail_fast: bool, id: u64)
    requires retirement::node_rules(w, n), node_headroom(n), n.phase == Phase::Reviewing,
        id < u64::MAX, port_review_reply::transition(port_review_reply::ended(n), fail_fast, id, Reply::ChangesRequested).1 <= w.next_question,
    ensures retirement::node_rules(w, port_review_reply::transition(port_review_reply::ended(n), fail_fast, id, Reply::ChangesRequested).0),
{
    reveal(port_review_reply::ended);
    reveal(retirement::node_rules);
    reveal(port_review_reply::transition);
    reveal(retirement::stopped);
    reveal(port_review_retry::round);
    reveal(port_review_retry::rung);
}


/// #806 reviewer row: preserve every complete-checker node rule.
pub(super) proof fn reviewer_rules(w: World, n: Node, fail_fast: bool, id: u64)
    requires retirement::node_rules(w, n), node_headroom(n), n.phase == Phase::Reviewing,
        id < u64::MAX, port_review_reply::transition(port_review_reply::ended(n), fail_fast, id, Reply::ReviewerOutOfRoom).1 <= w.next_question,
    ensures retirement::node_rules(w, port_review_reply::transition(port_review_reply::ended(n), fail_fast, id, Reply::ReviewerOutOfRoom).0),
{
    reveal(port_review_reply::ended);
    reveal(retirement::node_rules);
    reveal(port_review_reply::transition);
    reveal(retirement::stopped);
    reveal(port_review_retry::room);
    reveal(port_review_retry::rechecked);
}


/// #806 rebase_clean row: preserve every complete-checker node rule.
pub(super) proof fn rebase_clean_rules(w: World, n: Node, fail_fast: bool, id: u64, head: HeadId)
    requires retirement::node_rules(w, n), node_headroom(n), n.phase == Phase::Rebasing,
        id < u64::MAX, port_review_reply::transition(port_review_reply::ended(n), fail_fast, id, Reply::RebaseClean { head }).1 <= w.next_question,
    ensures retirement::node_rules(w, port_review_reply::transition(port_review_reply::ended(n), fail_fast, id, Reply::RebaseClean { head }).0),
{
    reveal(port_review_reply::ended);
    reveal(retirement::node_rules);
    reveal(port_review_reply::transition);
    reveal(retirement::stopped);
    reveal(port_review_retry::rechecked);
}


/// #806 rebase_needs row: preserve every complete-checker node rule.
pub(super) proof fn rebase_needs_rules(w: World, n: Node, fail_fast: bool, id: u64)
    requires retirement::node_rules(w, n), node_headroom(n), n.phase == Phase::Rebasing,
        id < u64::MAX, port_review_reply::transition(port_review_reply::ended(n), fail_fast, id, Reply::RebaseNeedsChanges).1 <= w.next_question,
    ensures retirement::node_rules(w, port_review_reply::transition(port_review_reply::ended(n), fail_fast, id, Reply::RebaseNeedsChanges).0),
{
    reveal(port_review_reply::ended);
    reveal(retirement::node_rules);
    reveal(port_review_reply::transition);
    reveal(retirement::stopped);
}

}
