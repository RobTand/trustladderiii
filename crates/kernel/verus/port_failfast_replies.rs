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

/// Q54 / #806: one world's accepted reply reduced to the shared retry relation of
/// one literal retry site, its world frame and fast-mode output length.
pub(super) open spec fn retry_side(w: World, after: World, kind: u8, step: ReviewStop,
    initial: Outbox, out: Outbox) -> bool {
    port_failfast_pair_frame::frame(w, after)
        && port_failfast_pair::retry_post(port_review_reply::ended(w.nodes@[0]),
            (after.nodes@[0], after.next_question), w.fail_fast, w.next_question, kind, step)
        && (w.fail_fast && (kind != 3 || w.nodes@[0].round != 0) ==> out.len == initial.len)
}

/// #806 probe: a pair equals a tuple whose projections it repeats.
pub(super) proof fn eta(t: (Node, u64), a: Node, b: u64)
    requires a == t.0, b == t.1,
    ensures (a, b) == t,
{
}

/// Maker ladder row / Q54: MakerCantMeet in one world.
pub(super) proof fn cant_meet_side(w: World, after: World, generation: u64, reply: Reply,
    initial: Outbox, out: Outbox)
    requires retirement::domain(w), selected(w.nodes@[0], reply), reply == Reply::MakerCantMeet,
        port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], reply),
        port_review_reply::reply_post(w, after, 0, generation, reply, initial, out),
    ensures retry_side(w, after, 0, ReviewStop::MakerCantMeet, initial, out),
{
    admitted(w, after, generation, reply, initial, out);
    reveal(port_review_reply::reply_post); reveal(port_review_reply::accepted_reply);
    reveal(port_replies_maker::transition_post); reveal(port_replies_maker::work_result);
    reveal(port_review_retry::rung);
}

/// Maker exhaustion row / Q54: MakerOutOfRoom in one world.
pub(super) proof fn maker_room_side(w: World, after: World, generation: u64, reply: Reply,
    initial: Outbox, out: Outbox)
    requires retirement::domain(w), selected(w.nodes@[0], reply), reply == Reply::MakerOutOfRoom,
        port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], reply),
        port_review_reply::reply_post(w, after, 0, generation, reply, initial, out),
    ensures retry_side(w, after, 1, ReviewStop::MakerOutOfRoom, initial, out),
{
    admitted(w, after, generation, reply, initial, out);
    reveal(port_review_reply::reply_post); reveal(port_review_reply::accepted_reply);
    reveal(port_replies_maker::transition_post); reveal(port_replies_maker::work_result);
    reveal(port_review_retry::room);
}

/// Review fix-round row / Q54: ChangesRequested in one world.
pub(super) proof fn changes_side(w: World, after: World, generation: u64, reply: Reply,
    initial: Outbox, out: Outbox)
    requires retirement::domain(w), selected(w.nodes@[0], reply), reply == Reply::ChangesRequested,
        port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], reply),
        port_review_reply::reply_post(w, after, 0, generation, reply, initial, out),
    ensures retry_side(w, after, 3, ReviewStop::ChangesRequested, initial, out),
{
    admitted(w, after, generation, reply, initial, out);
    reveal(port_review_reply::reply_post); reveal(port_review_reply::transition);
    reveal(port_review_retry::round); reveal(port_review_retry::rung);
}

/// Repeated reviewer exhaustion row / Q54: ReviewerOutOfRoom after a restart.
pub(super) proof fn reviewer_side(w: World, after: World, generation: u64, reply: Reply,
    initial: Outbox, out: Outbox)
    requires retirement::domain(w), selected(w.nodes@[0], reply), reply == Reply::ReviewerOutOfRoom,
        w.nodes@[0].review_restarted,
        port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], reply),
        port_review_reply::reply_post(w, after, 0, generation, reply, initial, out),
    ensures retry_side(w, after, 1, ReviewStop::ReviewerOutOfRoom, initial, out),
{
    admitted(w, after, generation, reply, initial, out);
    reveal(port_review_reply::reply_post); reveal(port_review_reply::transition); reveal(port_review_retry::room);
}

/// Failed checks row / Q54: ChecksDone without a pass in footprint, in one world.
pub(super) proof fn checks_side(w: World, after: World, generation: u64, reply: Reply,
    initial: Outbox, out: Outbox)
    requires retirement::domain(w), selected(w.nodes@[0], reply), reply is ChecksDone,
        !(match reply { Reply::ChecksDone { pass, in_footprint, .. } => pass && in_footprint, _ => false }),
        port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], reply),
        port_review_reply::reply_post(w, after, 0, generation, reply, initial, out),
    ensures retry_side(w, after, 3, ReviewStop::ChecksFailed, initial, out),
{
    admitted(w, after, generation, reply, initial, out);
    reveal(port_review_reply::reply_post); reveal(port_review_reply::transition);
    reveal(port_review_retry::round); reveal(port_review_retry::rung);
}

/// Q54 free review rows: the first reviewer restart and passing checks never read fail_fast.
pub(super) proof fn free_side(w: World, after: World, generation: u64, reply: Reply,
    initial: Outbox, out: Outbox)
    requires retirement::domain(w), selected(w.nodes@[0], reply),
        (reply == Reply::ReviewerOutOfRoom && !w.nodes@[0].review_restarted)
            || (match reply { Reply::ChecksDone { pass, in_footprint, .. } => pass && in_footprint, _ => false }),
        port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], reply),
        port_review_reply::reply_post(w, after, 0, generation, reply, initial, out),
    ensures port_failfast_pair_frame::frame(w, after),
        after.nodes@[0] == port_review_reply::transition(
            port_review_reply::ended(w.nodes@[0]), false, w.next_question, reply).0,
        after.next_question == port_review_reply::transition(
            port_review_reply::ended(w.nodes@[0]), false, w.next_question, reply).1,
{
    admitted(w, after, generation, reply, initial, out);
    reveal(port_review_reply::reply_post); reveal(port_review_reply::transition);
}

/// Q54 / #440: compare two worlds' retry relations at one literal retry site,
/// transporting the flag-on input world to `before` once.
pub(super) proof fn retry_pair(before: World, fast: World, off: World, on: World,
    kind: u8, step: ReviewStop, initial: Outbox, off_out: Outbox, on_out: Outbox)
    requires before.nodes@.len() == 2, !before.fail_fast, fast.fail_fast,
        port_failfast_admission::same_world(before, fast),
        kind < 4, kind != 2, node_headroom(before.nodes@[0]), before.next_question < u64::MAX,
        !(before.nodes@[0].phase is Stopping || before.nodes@[0].phase is Closed), initial.len == 0,
        retry_side(before, off, kind, step, initial, off_out),
        retry_side(fast, on, kind, step, initial, on_out),
    ensures !port_failfast_admission::same_world(off, on) ==> {
        on.nodes@[1] == off.nodes@[1]
            && on.nodes@[0].credit == before.nodes@[0].credit
            && on.nodes@[0].phase == Phase::Stopping(Outcome::Stopped)
            && on.nodes@[0].review_stop == Some(step) && on_out.len == 0
            && on.nodes@[0].question.is_none()
            && port_failfast_pair::retried(before.nodes@[0], off.nodes@[0])
    },
{
    assert(fast.nodes@[0] == before.nodes@[0]);
    assert(fast.nodes@[1] == before.nodes@[1]);
    port_failfast_pair_frame::compare(before, off, on, port_review_reply::ended(before.nodes@[0]),
        kind, step, on_out);
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
    if reply == Reply::MakerCantMeet {
        cant_meet_side(before, off, generation, reply, initial, off_out);
        cant_meet_side(fast, on, generation, reply, initial, on_out);
        retry_pair(before, fast, off, on, 0, ReviewStop::MakerCantMeet, initial, off_out, on_out);
    } else {
        maker_room_side(before, off, generation, reply, initial, off_out);
        maker_room_side(fast, on, generation, reply, initial, on_out);
        retry_pair(before, fast, off, on, 1, ReviewStop::MakerOutOfRoom, initial, off_out, on_out);
    }
}

/// Q54 review and checks rows: free recheck/pass paths are identical; later rounds
/// (kind 3) and repeated reviewer exhaustion (kind 1) use the shared comparison.
pub(super) proof fn ordinary_review(before: World, fast: World, off: World, on: World, generation: u64,
    reply: Reply, initial: Outbox, off_out: Outbox, on_out: Outbox)
    requires retirement::domain(before), !before.fail_fast,
        fast.fail_fast, port_failfast_admission::same_world(before, fast),
        selected(before.nodes@[0], reply),
        matches!(reply, Reply::ChangesRequested | Reply::ReviewerOutOfRoom | Reply::ChecksDone { .. }),
        node_headroom(before.nodes@[0]), before.next_question < u64::MAX, initial.len == 0,
        port_review_reply::active(before.nodes@[0], generation),
        port_review_reply::accepted_reply(before, before.nodes@[0], reply),
        port_review_reply::reply_post(before, off, 0, generation, reply, initial, off_out),
        port_review_reply::reply_post(fast, on, 0, generation, reply, initial, on_out),
    ensures diverted(before, off, on, reply, on_out),
{
    reveal(port_review_reply::accepted_reply);
    let n = before.nodes@[0];
    if (reply == Reply::ReviewerOutOfRoom && !n.review_restarted)
        || (match reply { Reply::ChecksDone { pass, in_footprint, .. } => pass && in_footprint, _ => false }) {
        free_side(before, off, generation, reply, initial, off_out);
        free_side(fast, on, generation, reply, initial, on_out);
        assert(fast.nodes@[0] == n && fast.nodes@[1] == before.nodes@[1]);
        assert(off.nodes@ =~= on.nodes@);
        assert(port_failfast_admission::same_world(off, on));
    } else if reply == Reply::ChangesRequested {
        changes_side(before, off, generation, reply, initial, off_out);
        changes_side(fast, on, generation, reply, initial, on_out);
        retry_pair(before, fast, off, on, 3, ReviewStop::ChangesRequested, initial, off_out, on_out);
    } else if reply == Reply::ReviewerOutOfRoom {
        reviewer_side(before, off, generation, reply, initial, off_out);
        reviewer_side(fast, on, generation, reply, initial, on_out);
        retry_pair(before, fast, off, on, 1, ReviewStop::ReviewerOutOfRoom, initial, off_out, on_out);
    } else {
        checks_side(before, off, generation, reply, initial, off_out);
        checks_side(fast, on, generation, reply, initial, on_out);
        retry_pair(before, fast, off, on, 3, ReviewStop::ChecksFailed, initial, off_out, on_out);
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
