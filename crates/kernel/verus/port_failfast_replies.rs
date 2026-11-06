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

/// Q54: the shared comparison kind of each non-file work-retry site.
pub(super) open spec fn kind(reply: Reply) -> u8 {
    match reply {
        Reply::MakerCantMeet => 0u8,
        Reply::MakerOutOfRoom | Reply::ReviewerOutOfRoom => 1u8,
        _ => 3u8,
    }
}

/// Q54: free review paths that never retry work: the first reviewer restart and passing checks.
pub(super) open spec fn free(reply: Reply, source: Node) -> bool {
    (reply == Reply::ReviewerOutOfRoom && !source.review_restarted)
        || match reply { Reply::ChecksDone { pass, in_footprint, .. } => pass && in_footprint, _ => false }
}

/// Q54 / #806: one accepted non-file reply, reduced to the shared retry relation
/// (or the flag-independent free-path result), its world frame and fast output length.
pub(super) open spec fn side(w: World, after: World, reply: Reply, initial: Outbox, out: Outbox) -> bool {
    let source = port_review_reply::ended(w.nodes@[0]);
    port_failfast_pair_frame::frame(w, after)
        && (w.fail_fast && !free(reply, source) && (kind(reply) != 3 || source.round != 0)
            ==> out.len == initial.len)
        && if free(reply, source) {
            after.nodes@[0] == port_review_reply::transition(source, false, w.next_question, reply).0
                && after.next_question == port_review_reply::transition(source, false, w.next_question, reply).1
        } else {
            port_failfast_pair::retry_post(source, (after.nodes@[0], after.next_question),
                w.fail_fast, w.next_question, kind(reply), reason(reply))
        }
}

/// Q54 maker ladder and exhaustion rows: extract side() from one world's real reply.
pub(super) proof fn maker_side(w: World, after: World, generation: u64, reply: Reply,
    initial: Outbox, out: Outbox)
    requires retirement::domain(w), selected(w.nodes@[0], reply), matches!(reply, Reply::MakerCantMeet | Reply::MakerOutOfRoom),
        port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], reply),
        port_review_reply::reply_post(w, after, 0, generation, reply, initial, out),
    ensures side(w, after, reply, initial, out),
{
    admitted(w, after, generation, reply, initial, out);
    reveal(port_review_reply::accepted_reply); reveal(port_review_reply::reply_post);
    reveal(port_replies_maker::transition_post); reveal(port_replies_maker::work_result);
    reveal(port_review_retry::rung); reveal(port_review_retry::room);
}

/// Q54 review and checks rows: extract side() from one world's real reply.
pub(super) proof fn review_side(w: World, after: World, generation: u64, reply: Reply,
    initial: Outbox, out: Outbox)
    requires retirement::domain(w), selected(w.nodes@[0], reply), matches!(reply, Reply::ChangesRequested | Reply::ReviewerOutOfRoom | Reply::ChecksDone { .. }),
        port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], reply),
        port_review_reply::reply_post(w, after, 0, generation, reply, initial, out),
    ensures side(w, after, reply, initial, out),
{
    admitted(w, after, generation, reply, initial, out);
    reveal(port_review_reply::reply_post); reveal(port_review_reply::transition);
    reveal(port_review_retry::round); reveal(port_review_retry::rung); reveal(port_review_retry::room);
}

/// Q54 maker ladder and exhaustion rows: MakerCantMeet (kind 0), MakerOutOfRoom (kind 1).
pub(super) proof fn ordinary_maker(before: World, fast: World, off: World, on: World, generation: u64,
    reply: Reply, initial: Outbox, off_out: Outbox, on_out: Outbox)
    requires retirement::domain(before), !before.fail_fast,
        fast.fail_fast, port_failfast_admission::same_world(before, fast),
        selected(before.nodes@[0], reply), matches!(reply, Reply::MakerCantMeet | Reply::MakerOutOfRoom),
        node_headroom(before.nodes@[0]), before.next_question < u64::MAX, initial.len == 0,
        port_review_reply::active(before.nodes@[0], generation),
        port_review_reply::accepted_reply(before, before.nodes@[0], reply),
        port_review_reply::reply_post(before, off, 0, generation, reply, initial, off_out),
        port_review_reply::reply_post(fast, on, 0, generation, reply, initial, on_out),
    ensures diverted(before, off, on, reply, on_out),
{
    reveal(port_review_reply::accepted_reply);
    maker_side(before, off, generation, reply, initial, off_out);
    maker_side(fast, on, generation, reply, initial, on_out);
    port_failfast_pair_frame::compare(before, off, on, port_review_reply::ended(before.nodes@[0]),
        kind(reply), reason(reply), on_out);
}

/// Q54 review and checks rows: free recheck/pass paths are identical; later rounds
/// (kind 3) and repeated reviewer exhaustion (kind 1) use the shared comparison.
pub(super) proof fn ordinary_review(before: World, fast: World, off: World, on: World, generation: u64,
    reply: Reply, initial: Outbox, off_out: Outbox, on_out: Outbox)
    requires retirement::domain(before), !before.fail_fast,
        fast.fail_fast, port_failfast_admission::same_world(before, fast),
        selected(before.nodes@[0], reply), matches!(reply, Reply::ChangesRequested | Reply::ReviewerOutOfRoom | Reply::ChecksDone { .. }),
        node_headroom(before.nodes@[0]), before.next_question < u64::MAX, initial.len == 0,
        port_review_reply::active(before.nodes@[0], generation),
        port_review_reply::accepted_reply(before, before.nodes@[0], reply),
        port_review_reply::reply_post(before, off, 0, generation, reply, initial, off_out),
        port_review_reply::reply_post(fast, on, 0, generation, reply, initial, on_out),
    ensures diverted(before, off, on, reply, on_out),
{
    reveal(port_review_reply::accepted_reply);
    review_side(before, off, generation, reply, initial, off_out);
    review_side(fast, on, generation, reply, initial, on_out);
    let source = port_review_reply::ended(before.nodes@[0]);
    if free(reply, source) {
        assert(off.nodes@ =~= on.nodes@);
        assert(port_failfast_admission::same_world(off, on));
    } else {
        port_failfast_pair_frame::compare(before, off, on, source, kind(reply), reason(reply), on_out);
    }
}

/// Q54: reduce all non-file reply paths to the existing parametric retry comparison.
pub(super) proof fn ordinary(before: World, fast: World, off: World, on: World, generation: u64,
    reply: Reply, initial: Outbox, off_out: Outbox, on_out: Outbox)
    requires retirement::domain(before), retirement::accepted(before), !before.fail_fast,
        fast.fail_fast, port_failfast_admission::same_world(before, fast),
        selected(before.nodes@[0], reply), !(reply is MakerNeedsFiles),
        node_headroom(before.nodes@[0]), before.next_question < u64::MAX, initial.len == 0,
        port_review_reply::active(before.nodes@[0], generation),
        port_review_reply::accepted_reply(before, before.nodes@[0], reply),
        port_review_reply::reply_post(before, off, 0, generation, reply, initial, off_out),
        port_review_reply::reply_post(fast, on, 0, generation, reply, initial, on_out),
    ensures diverted(before, off, on, reply, on_out),
{
    if matches!(reply, Reply::MakerCantMeet | Reply::MakerOutOfRoom) {
        ordinary_maker(before, fast, off, on, generation, reply, initial, off_out, on_out);
    } else {
        ordinary_review(before, fast, off, on, generation, reply, initial, off_out, on_out);
    }
}
}
