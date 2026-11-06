//! #806: review-specific assertions retained from the original Kani harnesses.
use super::*;

verus! {
/// Rebase progress row: exactly the executable balance plus remaining free rechecks.
pub(super) open spec fn fuel(n: Node) -> int {
    (if n.credit.withheld { 0int } else { n.credit.start as int + n.credit.earned as int
        + n.credit.added as int - n.credit.spent as int })
        + if n.rechecks < FREE_RECHECKS { FREE_RECHECKS as int - n.rechecks as int } else { 0int }
}

/// State-table RebaseClean: consume a free recheck, charge once, or ask Rob.
pub(super) proof fn clean_fuel(n: Node, after: Node, head: HeadId, fail_fast: bool, id: u64)
    requires n.phase == Phase::Rebasing, !n.credit.withheld, node_headroom(n),
        after == port_review_reply::transition(port_review_reply::ended(n), fail_fast, id,
            Reply::RebaseClean { head }).0,
    ensures after.phase == Phase::AwaitingRob || fuel(after) < fuel(n),
{
    reveal(port_review_reply::ended);
    reveal(port_review_reply::transition);
    reveal(port_review_retry::rechecked);
}

/// State-table RebaseNeedsChanges: charge even with free allowance, or ask Rob.
pub(super) proof fn needs_fuel(n: Node, after: Node, fail_fast: bool, id: u64)
    requires n.phase == Phase::Rebasing, !n.credit.withheld, node_headroom(n),
        after == port_review_reply::transition(port_review_reply::ended(n), fail_fast, id,
            Reply::RebaseNeedsChanges).0,
    ensures after.phase == Phase::AwaitingRob || fuel(after) < fuel(n),
{
    reveal(port_review_reply::ended);
    reveal(port_review_reply::transition);
}

/// ReviewerOutOfRoom: every assertion in check_apply's special reviewer block.
pub(super) open spec fn reviewer_post(n: Node, after: Node, fail_fast: bool) -> bool {
    after.run_failures == 0 && after.attempt == n.attempt && after.round == n.round
        && if n.review_restarted && fail_fast {
            after.phase == Phase::Stopping(Outcome::Stopped)
                && after.review_stop == Some(ReviewStop::ReviewerOutOfRoom) && after.credit == n.credit
        } else if n.review_restarted {
            !after.holds && if port_review_retry::funded(n) {
                after.phase == Phase::Planning { must_split: true }
                    && after.credit.spent == n.credit.spent + 1
            } else { after.phase == Phase::AwaitingRob && match after.question {
                Some(q) => q.cont == Cont::Planning { must_split: true }, None => false,
            } }
        } else {
            after.review_restarted
                && after.rechecks == if n.rechecks < u8::MAX { (n.rechecks + 1) as u8 } else { u8::MAX }
                && after.checked == n.checked
                && if after.rechecks <= FREE_RECHECKS || port_review_retry::funded(n) {
                    after.phase == Phase::Reviewing && after.generation > n.generation
                        && after.credit.spent == n.credit.spent
                            + if after.rechecks > FREE_RECHECKS { 1int } else { 0int }
                } else { after.phase == Phase::AwaitingRob && match after.question {
                    Some(q) => q.cont == Cont::Ready(Resume::Reviewing), None => false,
                } }
        }
}

/// Reviewer rows: the exact accepted update implies all original special assertions.
pub(super) proof fn reviewer_details(n: Node, after: Node, fail_fast: bool, id: u64)
    requires n.phase == Phase::Reviewing, node_headroom(n),
        after == port_review_reply::transition(port_review_reply::ended(n), fail_fast, id,
            Reply::ReviewerOutOfRoom).0,
    ensures reviewer_post(n, after, fail_fast),
{
    reveal(port_review_reply::transition); reveal(port_review_retry::room);
    reveal(port_review_retry::rechecked); reveal(retirement::stopped);
}

/// #806: the exact rebase applicability gate and the reviewer special-case gate.
#[verifier::opaque]
pub(super) open spec fn specific_post(before: World, after: World, generation: u64, reply: Reply) -> bool {
    let n = before.nodes@[0];
    let m = after.nodes@[0];
    current_item(n) == Some(Item::Await) && n.run == Some(generation) ==> match reply {
        Reply::ReviewerOutOfRoom => n.phase == Phase::Reviewing ==> reviewer_post(n, m, before.fail_fast),
        Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges => n.phase == Phase::Rebasing
            ==> m.phase == Phase::AwaitingRob || fuel(m) < fuel(n),
        _ => true,
    }
}

/// #806: transfer the special assertions through the real arbitrary-generation reply relation.
pub(super) proof fn specifics(before: World, after: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires !before.nodes@[0].credit.withheld, node_headroom(before.nodes@[0]),
        port_review_reply::reply_post(before, after, 0, generation, reply, out_before, out_after),
    ensures specific_post(before, after, generation, reply),
{
    reveal(specific_post); reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
    reveal(port_review_reply::accepted_reply); reveal(port_review_reply::transition_post);
    let n = before.nodes@[0];
    let m = after.nodes@[0];
    if current_item(n) == Some(Item::Await) && n.run == Some(generation) {
        match reply {
            Reply::ReviewerOutOfRoom => if n.phase == Phase::Reviewing {
                reviewer_details(n, m, before.fail_fast, before.next_question);
            },
            Reply::RebaseClean { head } => if n.phase == Phase::Rebasing {
                clean_fuel(n, m, head, before.fail_fast, before.next_question);
            },
            Reply::RebaseNeedsChanges => if n.phase == Phase::Rebasing {
                needs_fuel(n, m, before.fail_fast, before.next_question);
            },
            _ => {},
        }
    }
}

}
