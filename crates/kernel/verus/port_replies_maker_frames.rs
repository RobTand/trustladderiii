//! #806: complete local safety frames for the four maker replies.
use super::port_replies_maker::*;
use super::*;

verus! {
/// #806: same or weaker original-domain assumptions for an admitted maker node.
pub(super) open spec fn context(w: World, a: World, n: Node) -> bool {
    retirement::node_rules(w, n) && node_headroom(n) && !n.credit.withheld
        && n.phase == Phase::Making && current_item(n) == Some(Item::Await)
        && w.next_question < u64::MAX && a.gates@ == w.gates@
        && a.file_sets@ == w.file_sets@ && a.next_ticket == w.next_ticket
        && a.next_question >= w.next_question
}

/// #806: complete checker, shared Q54 assertions and unchanged resource ownership.
pub(super) open spec fn result(w: World, a: World, n: Node, next: Node, reply: Reply) -> bool {
    retirement::node_rules(a, next)
        && port_replies_frames::node_change(n, next, w.fail_fast, reply)
        && !next.credit.withheld && next.repo == n.repo
        && (next.holds ==> n.holds && next.footprint == n.footprint)
        && !next.pool && next.ticket.is_none()
}

/// MakerDone / #806: exact head update, phase change and every unchanged node field.
pub(super) proof fn done(w: World, a: World, n: Node, head: HeadId)
    requires context(w, a, n),
        a.next_question == w.next_question,
    ensures result(w, a, n, work_result(port_review_reply::ended(n), w.fail_fast,
        w.next_question, Reply::MakerDone { head }).0, Reply::MakerDone { head }),
{
    reveal(retirement::node_rules); reveal(work_result);
    footprint::storage_frame(w, a, n.footprint, n.footprint);
}

/// MakerCantMeet / #806: exact ladder frame, fresh questions and Q54 stopping.
pub(super) proof fn cant_meet(w: World, a: World, n: Node)
    requires context(w, a, n),
        a.next_question == work_result(port_review_reply::ended(n), w.fail_fast,
            w.next_question, Reply::MakerCantMeet).1,
    ensures result(w, a, n, work_result(port_review_reply::ended(n), w.fail_fast,
        w.next_question, Reply::MakerCantMeet).0, Reply::MakerCantMeet),
{
    reveal(retirement::node_rules); reveal(work_result); reveal(port_review_retry::rung);
    reveal(retirement::stopped);
    footprint::storage_frame(w, a, n.footprint, n.footprint);
}

/// MakerOutOfRoom / #806: exact exhaustion frame, credit and Q54 stopping.
pub(super) proof fn out_of_room(w: World, a: World, n: Node)
    requires context(w, a, n),
        a.next_question == work_result(port_review_reply::ended(n), w.fail_fast,
            w.next_question, Reply::MakerOutOfRoom).1,
    ensures result(w, a, n, work_result(port_review_reply::ended(n), w.fail_fast,
        w.next_question, Reply::MakerOutOfRoom).0, Reply::MakerOutOfRoom),
{
    reveal(retirement::node_rules); reveal(work_result); reveal(port_review_retry::room);
    reveal(retirement::stopped);
    footprint::storage_frame(w, a, n.footprint, n.footprint);
}

/// MakerNeedsFiles / #806: original inline inputs never change file storage.
pub(super) proof fn inline_files(w: World, a: World, n: Node, left: u64, right: u64,
    fits: bool, out_before: Outbox, out_after: Outbox)
    requires n.footprint == footprint::inline_footprint(left),
        files_post(w, a, 0, port_review_reply::ended(n), footprint::inline_footprint(right),
            fits, out_before, out_after),
    ensures a.file_sets@ == w.file_sets@,
        (fits && !footprint::footprint_empty(footprint::inline_footprint(right))
            && (port_review_retry::funded(n) || w.fail_fast)
            ==> a.nodes@[0].footprint == footprint::inline_footprint(left | right)
                && !footprint::footprint_empty(a.nodes@[0].footprint)),
{
    reveal(files_post); reveal(union_post);
}

/// MakerNeedsFiles / Q54: funded extensions, credit questions and exhaustion
/// preserve the complete node checker and every original shared assertion.
pub(super) proof fn needs_files(w: World, a: World, n: Node, left: u64, right: u64,
    fits: bool, out_before: Outbox, out_after: Outbox)
    requires context(w, a, n), n.footprint == footprint::inline_footprint(left),
        files_post(w, a, 0, port_review_reply::ended(n), footprint::inline_footprint(right),
            fits, out_before, out_after),
    ensures result(w, a, n, a.nodes@[0], Reply::MakerNeedsFiles {
        files: footprint::inline_footprint(right), fits }),
{
    inline_files(w, a, n, left, right, fits, out_before, out_after);
    footprint::inline_is_valid(a, left | right);
    footprint::storage_frame(w, a, n.footprint, n.footprint);
    reveal(files_post); reveal(retirement::node_rules); reveal(port_review_retry::room);
    reveal(retirement::stopped);
}
}

verus! {
/// MakerNeedsFiles / Q54: a fitting unfunded fast extension asks for credit,
/// retains the joined footprint, and preserves every credit field.
pub(super) proof fn funding(w: World, a: World, generation: u64, files: Footprint,
    fits: bool, out_before: Outbox, out_after: Outbox)
    requires retirement::domain(w), retirement::accepted(w), w.fail_fast,
        w.nodes@[0].phase == Phase::Making,
        current_item(w.nodes@[0]) == Some(Item::Await), w.nodes@[0].run == Some(generation),
        footprint::valid_footprint(w, files), fits, !footprint::footprint_empty(files),
        !port_review_retry::funded(w.nodes@[0]),
        port_review_reply::reply_post(w, a, 0, generation,
            Reply::MakerNeedsFiles { files, fits }, out_before, out_after),
    ensures a.nodes@[0].phase == Phase::AwaitingRob,
        a.nodes@[0].question.is_some(),
        a.nodes@[0].question.unwrap().kind == QKind::OutOfCredit,
        a.nodes@[0].question.unwrap().cont == Cont::Ready(Resume::Making),
        a.nodes@[0].review_stop.is_none(), a.nodes@[0].credit == w.nodes@[0].credit,
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
    reveal(port_review_reply::transition_post); reveal(port_replies_contracts::transition_post);
    reveal(transition_post); reveal(files_post); reveal(port_review_reply::accepted_reply);
    reveal(retirement::accepted); reveal(retirement::node_rules);
}
}
