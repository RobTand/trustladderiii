//! #806: complete maker safety through the real admission relation.
use super::*;

verus! {
/// #806: retain every original four-file input, allowing all 64 inline file bits.
/// Non-file payloads impose no footprint restriction.
pub(super) open spec fn inline_input(w: World, reply: Reply) -> bool {
    if let Reply::MakerNeedsFiles { files, .. } = reply {
        w.nodes@[0].phase == Phase::Making && footprint::valid_footprint(w, files) ==>
            exists|left: u64, right: u64| w.nodes@[0].footprint == footprint::inline_footprint(left)
                && files == footprint::inline_footprint(right)
    } else { true }
}

/// Maker retry rows / #806: the exact ladder and exhaustion allocate at most one question.
pub(super) proof fn work_question(n: Node, fast: bool, id: u64, reply: Reply)
    requires id < u64::MAX,
    ensures port_replies_maker::work_result(n, fast, id, reply).1 >= id,
        port_replies_maker::work_result(n, fast, id, reply).1 as int <= id as int + 1,
{
    reveal(port_replies_maker::work_result); reveal(port_review_retry::rung);
    reveal(port_review_retry::room);
}

/// MakerNeedsFiles / #806: the funded branch or credit question is monotone.
pub(super) proof fn files_question(w: World, a: World, n: Node, files: Footprint, fits: bool,
    out_before: Outbox, out_after: Outbox)
    requires w.next_question < u64::MAX,
        port_replies_maker::files_post(w, a, 0, n, files, fits, out_before, out_after),
    ensures a.next_question >= w.next_question,
        a.next_question as int <= w.next_question as int + 1,
        retirement::notices_only(out_before) ==> retirement::notices_only(out_after),
{
    reveal(port_replies_maker::files_post); reveal(port_review_retry::room);
}

/// #806 maker rows: derive only the local input facts from the complete world checker.
pub(super) proof fn source_context(w: World, generation: u64, reply: Reply)
    requires retirement::domain(w), retirement::accepted(w), retirement::bounded(w),
        port_replies_maker::selected(reply), port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], reply),
    ensures retirement::node_rules(w, w.nodes@[0]), node_headroom(w.nodes@[0]),
        !w.nodes@[0].credit.withheld, w.nodes@[0].phase == Phase::Making,
        current_item(w.nodes@[0]) == Some(Item::Await), w.next_question < u64::MAX,
{
    reveal(retirement::accepted); reveal(retirement::bounded);
    reveal(port_review_reply::accepted_reply);
}

/// #806 MakerDone: lift this exact frame through real admission, in one bounded query.
pub(super) proof fn accepted_done(w: World, a: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires reply is MakerDone, retirement::domain(w), retirement::accepted(w), retirement::bounded(w),
        port_replies_maker::selected(reply), inline_input(w, reply),
        !(w.nodes@[0].phase is Stopping), port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], reply),
        retirement::notices_only(out_before),
        port_review_reply::reply_post(w, a, 0, generation, reply, out_before, out_after),
    ensures port_replies_admission::node_result(w, a, reply),
        port_replies_admission::table_frame(w, a), retirement::notices_only(out_after),
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
    reveal(port_review_reply::transition_post); reveal(port_replies_contracts::transition_post);
    reveal(port_replies_maker::transition_post); reveal(port_review_reply::accepted_reply);
    source_context(w, generation, reply);
    reveal(port_replies_admission::node_result);
    let n = w.nodes@[0];
    let source = port_review_reply::ended(n);
    if let Reply::MakerDone { head } = reply {
        work_question(source, w.fail_fast, w.next_question, reply);
        assert(a.next_question == w.next_question) by { reveal(port_replies_maker::work_result); }
        port_replies_maker_frames::done(w, a, n, head);
    }

}

/// #806 MakerCantMeet: lift this exact frame through real admission, in one bounded query.
pub(super) proof fn accepted_cant(w: World, a: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires reply is MakerCantMeet, retirement::domain(w), retirement::accepted(w), retirement::bounded(w),
        port_replies_maker::selected(reply), inline_input(w, reply),
        !(w.nodes@[0].phase is Stopping), port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], reply),
        retirement::notices_only(out_before),
        port_review_reply::reply_post(w, a, 0, generation, reply, out_before, out_after),
    ensures port_replies_admission::node_result(w, a, reply),
        port_replies_admission::table_frame(w, a), retirement::notices_only(out_after),
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
    reveal(port_review_reply::transition_post); reveal(port_replies_contracts::transition_post);
    reveal(port_replies_maker::transition_post); reveal(port_review_reply::accepted_reply);
    source_context(w, generation, reply);
    reveal(port_replies_admission::node_result);
    let n = w.nodes@[0];
    let source = port_review_reply::ended(n);
    work_question(source, w.fail_fast, w.next_question, reply);
    port_replies_maker_frames::cant_meet(w, a, n);

}

/// #806 MakerOutOfRoom: lift this exact frame through real admission, in one bounded query.
pub(super) proof fn accepted_room(w: World, a: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires reply is MakerOutOfRoom, retirement::domain(w), retirement::accepted(w), retirement::bounded(w),
        port_replies_maker::selected(reply), inline_input(w, reply),
        !(w.nodes@[0].phase is Stopping), port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], reply),
        retirement::notices_only(out_before),
        port_review_reply::reply_post(w, a, 0, generation, reply, out_before, out_after),
    ensures port_replies_admission::node_result(w, a, reply),
        port_replies_admission::table_frame(w, a), retirement::notices_only(out_after),
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
    reveal(port_review_reply::transition_post); reveal(port_replies_contracts::transition_post);
    reveal(port_replies_maker::transition_post); reveal(port_review_reply::accepted_reply);
    source_context(w, generation, reply);
    reveal(port_replies_admission::node_result);
    let n = w.nodes@[0];
    let source = port_review_reply::ended(n);
    work_question(source, w.fail_fast, w.next_question, reply);
    port_replies_maker_frames::out_of_room(w, a, n);

}

/// #806 MakerNeedsFiles: lift this exact frame through real admission, in one bounded query.
pub(super) proof fn accepted_files(w: World, a: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires reply is MakerNeedsFiles, retirement::domain(w), retirement::accepted(w), retirement::bounded(w),
        port_replies_maker::selected(reply), inline_input(w, reply),
        !(w.nodes@[0].phase is Stopping), port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], reply),
        retirement::notices_only(out_before),
        port_review_reply::reply_post(w, a, 0, generation, reply, out_before, out_after),
    ensures port_replies_admission::node_result(w, a, reply),
        port_replies_admission::table_frame(w, a), retirement::notices_only(out_after),
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
    reveal(port_review_reply::transition_post); reveal(port_replies_contracts::transition_post);
    reveal(port_replies_maker::transition_post); reveal(port_review_reply::accepted_reply);
    source_context(w, generation, reply);
    reveal(port_replies_admission::node_result);
    let n = w.nodes@[0];
    let source = port_review_reply::ended(n);
    if let Reply::MakerNeedsFiles { files, fits } = reply {
        let (left, right) = choose|left: u64, right: u64|
            n.footprint == footprint::inline_footprint(left)
                && files == footprint::inline_footprint(right);
        port_replies_maker_frames::inline_files(w, a, n, left, right, fits, out_before, out_after);
        files_question(w, a, source, files, fits, out_before, out_after);
        port_replies_maker_frames::needs_files(w, a, n, left, right, fits, out_before, out_after);
    }

}

/// #806: each accepted maker payload uses its own exact local frame lemma.
pub(super) proof fn accepted_reply(w: World, a: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires retirement::domain(w), retirement::accepted(w), retirement::bounded(w),
        port_replies_maker::selected(reply), inline_input(w, reply),
        !(w.nodes@[0].phase is Stopping), port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], reply),
        retirement::notices_only(out_before),
        port_review_reply::reply_post(w, a, 0, generation, reply, out_before, out_after),
    ensures port_replies_admission::node_result(w, a, reply),
        port_replies_admission::table_frame(w, a), retirement::notices_only(out_after),
{
    match reply {
        Reply::MakerDone { .. } => accepted_done(w, a, generation, reply, out_before, out_after),
        Reply::MakerCantMeet => accepted_cant(w, a, generation, reply, out_before, out_after),
        Reply::MakerOutOfRoom => accepted_room(w, a, generation, reply, out_before, out_after),
        Reply::MakerNeedsFiles { .. } => accepted_files(w, a, generation, reply, out_before, out_after),
        _ => {},
    }
}

/// #806: accepted, stale, stopping and rejected maker replies preserve the complete
/// checker and all shared check_nodes/check_outputs/Q54 assertions.
pub(super) proof fn reply_safe(w: World, a: World, generation: u64, reply: Reply,
    out_before: Outbox, out_after: Outbox)
    requires retirement::domain(w), retirement::accepted(w), retirement::bounded(w),
        port_replies_maker::selected(reply), inline_input(w, reply),
        retirement::notices_only(out_before),
        port_review_reply::reply_post(w, a, 0, generation, reply, out_before, out_after),
    ensures retirement::domain(a), retirement::accepted(a),
        port_replies_safety::shared(w, a, reply), port_replies_admission::table_frame(w, a),
        retirement::notices_only(out_after),
{
    let n = w.nodes@[0];
    if n.phase is Stopping {
        port_replies_admission::stopped_reply(w, a, generation, reply, out_before, out_after);
    } else if !port_review_reply::active(n, generation) {
        port_replies_admission::stale_reply(w, a, generation, reply, out_before, out_after);
    } else if !port_review_reply::accepted_reply(w, n, reply) {
        port_replies_admission::rejected_reply(w, a, generation, reply, out_before, out_after);
    } else {
        accepted_reply(w, a, generation, reply, out_before, out_after);
    }
    reveal(port_replies_admission::node_result);
    port_replies_frames::world_frame(w, a);
    assert forall|i: int| 0 <= i < 2 implies
        port_replies_frames::node_change(w.nodes@[i], #[trigger] a.nodes@[i], w.fail_fast, reply) by {
        assert(i == 0 || i == 1);
    }
}
}
