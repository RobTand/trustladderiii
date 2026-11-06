//! Q54 / #440 / #806: complete fast-mode checker composition for ChecksDone.
use super::*;

verus! {
/// ChecksDone / Q54: the accepted fast result is a checked review, a free first
/// fix round, or the same shared question-free stop used by every retry site.
pub(super) proof fn node(w: World, n: Node, after: Node, id: u64, pass: bool, in_footprint: bool)
    requires retirement::node_rules(w, n), node_headroom(n), n.phase == Phase::Checking,
        id < u64::MAX,
        after == port_review_reply::transition(port_review_reply::ended(n), true, id,
            Reply::ChecksDone { report: true, pass, in_footprint }).0,
    ensures retirement::node_rules(w, after),
        port_failfast_safety::node_post(n, after),
        after.repo == n.repo, after.footprint == n.footprint,
        !after.credit.withheld, after.holds ==> n.holds,
        after.pool ==> n.pool, after.ticket.is_some() ==> after.ticket == n.ticket,
{
    reveal(retirement::node_rules); reveal(port_review_reply::transition);
    reveal(port_review_retry::round); reveal(port_review_retry::rung); reveal(retirement::stopped);
}

/// ChecksDone / Q30/Q54: compose active, rejected-report and stale branches with
/// retirement's complete world/resource frame, using the real reply relation.
pub(super) proof fn world(before: World, after: World, generation: u64,
    report: bool, pass: bool, in_footprint: bool, initial: Outbox, out: Outbox)
    requires retirement::domain(before), retirement::accepted(before), retirement::bounded(before),
        before.fail_fast, before.nodes@[0].phase == Phase::Checking,
        retirement::notices_only(initial),
        port_review_reply::reply_post(before, after, 0, generation,
            Reply::ChecksDone { report, pass, in_footprint }, initial, out),
    ensures retirement::domain(after), retirement::accepted(after),
        retirement::notices_only(out),
        port_failfast_safety::node_post(before.nodes@[0], after.nodes@[0]),
{
    let reply = Reply::ChecksDone { report, pass, in_footprint };
    let n = before.nodes@[0];
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
    reveal(port_review_reply::accepted_reply); reveal(retirement::bounded);
    if !port_review_reply::active(n, generation) {
        assert(before.nodes@ =~= after.nodes@);
        port_failfast_admission::stale_world(before, after);
    } else if !report {
        port_replies_admission::rejected_reply(before, after, generation, reply, initial, out);
        reveal(port_replies_admission::node_result);
        port_replies_frames::world_frame(before, after);
        reveal(port_review_retry::failed_run); reveal(retirement::incident);
    } else {
        reveal(port_review_reply::transition_post); reveal(port_review_reply::transition);
        reveal(port_review_retry::round); reveal(port_review_retry::rung);
        reveal(retirement::accepted);
        port_review_frames::question_frame(before, after, n);
        node(after, n, after.nodes@[0], before.next_question, pass, in_footprint);
        port_replies_frames::world_frame(before, after);
    }
}
}
