//! #806: shared Kani node assertions and Q54 for the six review reply ports.
use super::*;

verus! {
/// #806 check_nodes and Q54: the full common assertion set for these replies.
#[verifier::opaque]
pub(super) open spec fn node_post(n: Node, after: Node, fail_fast: bool) -> bool {
    after.repo == n.repo && after.footprint == n.footprint
        && after.proposed == n.proposed && after.adopted == n.adopted
        && after.next_seq == n.next_seq
        && (n.phase is Closed ==> after.phase == n.phase)
        && after.credit.withheld == n.credit.withheld
        && after.credit.earned == n.credit.earned
        && after.credit.merge_credited == n.credit.merge_credited
        && (after.holds ==> n.holds) && (after.pool ==> n.pool)
        && (after.ticket.is_some() ==> after.ticket == n.ticket)
        && (if fail_fast {
            after.attempt <= if n.attempt < 1 { 1u8 } else { n.attempt }
                && (after.replanned ==> n.replanned)
                && (after.phase is Planning ==> n.phase is Planning || n.phase == Phase::AwaitingRob)
                && (match after.question {
                    Some(q) => q.kind == QKind::CantMeet ==> after.question == n.question,
                    None => true,
                })
        } else { after.review_stop == n.review_stop })
        && (after.review_stop != n.review_stop ==> n.review_stop.is_none()
            && after.phase == Phase::Stopping(Outcome::Stopped)
            && after.credit == n.credit && after.question.is_none())
}

/// Q30 and Q54: stale replies stutter and stopping replies only release ownership.
pub(super) proof fn inactive(n: Node, after: Node, generation: u64, fail_fast: bool)
    requires after == n || (n.phase is Stopping
        && after == port_review_reply::stopping_reply(n, generation)),
    ensures node_post(n, after, fail_fast),
{ reveal(node_post); }

/// Failed-run rows and Q54: the real failure update preserves all common assertions.
pub(super) proof fn rejected(n: Node, after: Node, fail_fast: bool)
    requires after == port_review_retry::failed_run(n),
        !(n.phase is Closed || n.phase is Stopping),
    ensures node_post(n, after, fail_fast),
{
    reveal(node_post); reveal(port_review_retry::failed_run); reveal(retirement::incident);
}

/// Review/rebase rows and Q54: exact admitted updates spend once or stop without spending.
pub(super) proof fn accepted(n: Node, after: Node, reply: Reply, fail_fast: bool, id: u64)
    requires port_review_reply::selected(reply), !(reply is ChecksDone), n.review_stop.is_none(),
        n.phase == Phase::Reviewing || n.phase == Phase::Rebasing,
        after == port_review_reply::transition(port_review_reply::ended(n), fail_fast, id, reply).0,
    ensures node_post(n, after, fail_fast),
{
    reveal(node_post); reveal(port_review_reply::transition); reveal(port_review_reply::ended);
    reveal(port_review_retry::round); reveal(port_review_retry::rung);
    reveal(port_review_retry::room); reveal(port_review_retry::rechecked); reveal(retirement::stopped);
}
}
