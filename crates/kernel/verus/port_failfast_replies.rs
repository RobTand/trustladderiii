//! Q54 / #440 / #806: one comparison over the published real reply contracts.
use super::*;

verus! {
/// Q54: the six remaining reply diversions, in their original accepting phases.
pub(super) open spec fn selected(n: Node, reply: Reply) -> bool {
    match reply {
        Reply::MakerCantMeet | Reply::MakerOutOfRoom | Reply::MakerNeedsFiles { .. } => n.phase == Phase::Making,
        Reply::ChangesRequested | Reply::ReviewerOutOfRoom => n.phase == Phase::Reviewing,
        Reply::ChecksDone { .. } => n.phase == Phase::Checking,
        _ => false,
    }
}

/// Q54: name the actual retry site, including the maker-file funding exception.
pub(super) open spec fn reason(reply: Reply) -> ReviewStop {
    match reply {
        Reply::MakerCantMeet => ReviewStop::MakerCantMeet,
        Reply::MakerOutOfRoom => ReviewStop::MakerOutOfRoom,
        Reply::MakerNeedsFiles { .. } => ReviewStop::MakerNeedsFiles,
        Reply::ChangesRequested => ReviewStop::ChangesRequested,
        Reply::ReviewerOutOfRoom => ReviewStop::ReviewerOutOfRoom,
        _ => ReviewStop::ChecksFailed,
    }
}

/// Q54: retain every original diverted assertion and its distinct funding branch.
pub(super) open spec fn diverted(before: World, off: World, on: World, reply: Reply, out: Outbox) -> bool {
    !port_failfast_admission::same_world(off, on) ==> {
        let n = on.nodes@[0]; let r = off.nodes@[0];
        on.nodes@[1] == off.nodes@[1] && n.credit == before.nodes@[0].credit
            && port_failfast_pair::retried(before.nodes@[0], r)
            && if match n.question { Some(q) => q.kind == QKind::OutOfCredit
                && q.cont == Cont::Ready(Resume::Making), None => false } {
                n.review_stop.is_none() && match r.question { Some(q) =>
                    q.kind == QKind::OutOfCredit && q.cont == (Cont::Planning { must_split: true }),
                    None => false }
            } else {
                n.phase == Phase::Stopping(Outcome::Stopped)
                    && n.review_stop == Some(reason(reply)) && out.len == 0
            }
    }
}

/// #832 / #832-A / #629: all flag-off comparisons use the current three-case rule.
pub(super) open spec fn stop_rule(before: Node, after: Node, fast: bool, reply: Reply) -> bool {
    !fast && after.review_stop != before.review_stop ==> {
        after.review_stop == Some(ReviewStop::PublicationBody)
            || (after.review_stop == Some(ReviewStop::AuditDisagreement)
                && reply == Reply::AuditDisagrees && before.phase == Phase::Auditing && before.audit_rounds >= 1)
            || (after.review_stop == Some(ReviewStop::PlanCantMeet)
                && matches!(reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit)
                && before.phase is Planning)
    }
}

/// Q54: expose only the accepted owner contract, keeping unrelated worker fields opaque.
pub(super) proof fn admitted(before: World, after: World, generation: u64, reply: Reply,
    initial: Outbox, out: Outbox)
    requires retirement::domain(before), selected(before.nodes@[0], reply),
        port_review_reply::active(before.nodes@[0], generation),
        port_review_reply::accepted_reply(before, before.nodes@[0], reply),
        port_review_reply::reply_post(before, after, 0, generation, reply, initial, out),
    ensures port_replies_maker::selected(reply) ==>
        port_replies_maker::transition_post(before, after, 0,
            port_review_reply::ended(before.nodes@[0]), reply, initial, out),
        !port_replies_maker::selected(reply) ==>
            port_review_reply::review_transition_post(before, after, 0,
                port_review_reply::ended(before.nodes@[0]), reply, initial, out),
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
    reveal(port_review_reply::transition_post); reveal(port_replies_contracts::transition_post);
}

/// Q54: reduce all non-file reply paths to the existing parametric retry comparison.
pub(super) proof fn ordinary(before: World, off: World, on: World, generation: u64, reply: Reply,
    initial: Outbox, off_out: Outbox, on_out: Outbox)
    requires retirement::domain(before), retirement::accepted(before), !before.fail_fast,
        selected(before.nodes@[0], reply), !(reply is MakerNeedsFiles),
        node_headroom(before.nodes@[0]), before.next_question < u64::MAX, initial.len == 0,
        port_review_reply::active(before.nodes@[0], generation),
        port_review_reply::accepted_reply(before, before.nodes@[0], reply),
        port_review_reply::reply_post(before, off, 0, generation, reply, initial, off_out),
        port_review_reply::reply_post(World { fail_fast: true, ..before }, on,
            0, generation, reply, initial, on_out),
    ensures diverted(before, off, on, reply, on_out),
{
    let fast = World { fail_fast: true, ..before };
    admitted(before, off, generation, reply, initial, off_out);
    admitted(fast, on, generation, reply, initial, on_out);
    reveal(port_review_reply::accepted_reply);
    reveal(port_replies_maker::transition_post); reveal(port_replies_maker::work_result);
    reveal(port_review_reply::transition); reveal(port_review_reply::reply_post);
    reveal(retirement::accepted); reveal(retirement::node_rules);
    let source = port_review_reply::ended(before.nodes@[0]);
    if (reply == Reply::ReviewerOutOfRoom && !source.review_restarted)
        || (match reply { Reply::ChecksDone { pass, in_footprint, .. } => pass && in_footprint, _ => false }) {
        assert(off.nodes@ =~= on.nodes@);
    } else {
        let kind = if reply == Reply::MakerCantMeet { 0u8 }
            else if matches!(reply, Reply::MakerOutOfRoom | Reply::ReviewerOutOfRoom) { 1u8 }
            else { 3u8 };
        reveal(port_review_retry::rung);
        port_failfast_pair_frame::compare(before, off, on, source, kind, reason(reply), on_out);
    }
}
}
