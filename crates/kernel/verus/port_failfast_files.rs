//! Q54 / #440 / #806: compare actual maker-file unions, credit questions and retries.
use super::*;

verus! {
/// Q54 funded file extension: both flags take the same paid union.
pub(super) proof fn joined_funded(before: World, fast: World, off: World, on: World, generation: u64,
    files: Footprint, fits: bool, initial: Outbox, off_out: Outbox, on_out: Outbox)
    requires retirement::domain(before), retirement::accepted(before), !before.fail_fast,
        fast.fail_fast, port_failfast_admission::same_world(before, fast),
        before.file_sets@.len() == 0, before.next_question < u64::MAX,
        before.nodes@[0].phase == Phase::Making, node_headroom(before.nodes@[0]), initial.len == 0,
        port_review_reply::active(before.nodes@[0], generation),
        footprint::valid_footprint(before, files),
        port_replies_maker_admission::inline_input(before, Reply::MakerNeedsFiles { files, fits }),
        port_review_reply::reply_post(before, off, 0, generation,
            Reply::MakerNeedsFiles { files, fits }, initial, off_out),
        port_review_reply::reply_post(fast, on,
            0, generation, Reply::MakerNeedsFiles { files, fits }, initial, on_out),
        fits, !footprint::footprint_empty(files),
        port_review_retry::funded(port_review_reply::ended(before.nodes@[0])),
    ensures port_failfast_replies::diverted(before, off, on, Reply::MakerNeedsFiles { files, fits }, on_out),
{
    let reply = Reply::MakerNeedsFiles { files, fits };
    let source = port_review_reply::ended(before.nodes@[0]);
    reveal(port_review_reply::accepted_reply);
    footprint::storage_frame(before, fast, files, files);
    port_failfast_replies::admitted(before, off, generation, reply, initial, off_out);
    port_failfast_replies::admitted(fast, on, generation, reply, initial, on_out);
    reveal(port_replies_maker::transition_post); reveal(port_replies_maker::files_post);
    reveal(port_review_reply::reply_post);
    reveal(port_replies_maker::union_post);
    reveal(retirement::accepted); reveal(retirement::node_rules);
    let left = choose|bits: u64| source.footprint == footprint::inline_footprint(bits);
    let right = choose|bits: u64| files == footprint::inline_footprint(bits);
    assert(on.nodes@[0].footprint == footprint::inline_footprint(left | right));
    assert(off.nodes@[0].footprint == on.nodes@[0].footprint);
    assert(off.nodes@ =~= on.nodes@);
    assert(port_failfast_admission::same_world(off, on));
}

/// Q54 unfunded file extension: fail_fast asks for maker credit; the ordinary
/// branch asks to split, which the original check counts as a retry.
pub(super) proof fn joined_unfunded(before: World, fast: World, off: World, on: World, generation: u64,
    files: Footprint, fits: bool, initial: Outbox, off_out: Outbox, on_out: Outbox)
    requires retirement::domain(before), retirement::accepted(before), !before.fail_fast,
        fast.fail_fast, port_failfast_admission::same_world(before, fast),
        before.file_sets@.len() == 0, before.next_question < u64::MAX,
        before.nodes@[0].phase == Phase::Making, node_headroom(before.nodes@[0]), initial.len == 0,
        port_review_reply::active(before.nodes@[0], generation),
        footprint::valid_footprint(before, files),
        port_replies_maker_admission::inline_input(before, Reply::MakerNeedsFiles { files, fits }),
        port_review_reply::reply_post(before, off, 0, generation,
            Reply::MakerNeedsFiles { files, fits }, initial, off_out),
        port_review_reply::reply_post(fast, on,
            0, generation, Reply::MakerNeedsFiles { files, fits }, initial, on_out),
        fits, !footprint::footprint_empty(files),
        !port_review_retry::funded(port_review_reply::ended(before.nodes@[0])),
    ensures port_failfast_replies::diverted(before, off, on, Reply::MakerNeedsFiles { files, fits }, on_out),
{
    let reply = Reply::MakerNeedsFiles { files, fits };
    let source = port_review_reply::ended(before.nodes@[0]);
    reveal(port_review_reply::accepted_reply);
    footprint::storage_frame(before, fast, files, files);
    port_failfast_replies::admitted(before, off, generation, reply, initial, off_out);
    port_failfast_replies::admitted(fast, on, generation, reply, initial, on_out);
    reveal(port_replies_maker::transition_post); reveal(port_replies_maker::files_post);
    reveal(port_review_reply::reply_post);
    reveal(port_replies_maker::union_post);
    reveal(retirement::accepted); reveal(retirement::node_rules);
    let left = choose|bits: u64| source.footprint == footprint::inline_footprint(bits);
    let right = choose|bits: u64| files == footprint::inline_footprint(bits);
    assert(on.nodes@[0].footprint == footprint::inline_footprint(left | right));
    port_failfast_pair::funding(source, on.nodes@[0].footprint, before.next_question);
    reveal(port_review_retry::room);
    assert(on.nodes@[0].credit == before.nodes@[0].credit);
    assert(port_failfast_pair::retried(before.nodes@[0], off.nodes@[0]));
}

/// Q54 MakerNeedsFiles non-fitting or empty input: one world's real reply
/// relation reduces to the exhaustion-row equation and an unchanged world frame.
pub(super) proof fn room_side(w: World, after: World, generation: u64, files: Footprint,
    fits: bool, initial: Outbox, out: Outbox)
    requires retirement::domain(w), w.nodes@[0].phase == Phase::Making,
        !(fits && !footprint::footprint_empty(files)),
        port_review_reply::active(w.nodes@[0], generation),
        port_review_reply::accepted_reply(w, w.nodes@[0], Reply::MakerNeedsFiles { files, fits }),
        port_review_reply::reply_post(w, after, 0, generation,
            Reply::MakerNeedsFiles { files, fits }, initial, out),
    ensures port_failfast_replies::retry_side(w, after, 1, ReviewStop::MakerNeedsFiles, initial, out),
{
    port_failfast_replies::admitted(w, after, generation, Reply::MakerNeedsFiles { files, fits }, initial, out);
    reveal(port_review_reply::accepted_reply);
    reveal(port_replies_maker::transition_post); reveal(port_replies_maker::files_post);
    reveal(port_review_reply::reply_post); reveal(port_review_retry::room);
}

/// Q54 MakerNeedsFiles non-fitting or empty input: the parametric room diversion.
pub(super) proof fn exhausted(before: World, fast: World, off: World, on: World, generation: u64,
    files: Footprint, fits: bool, initial: Outbox, off_out: Outbox, on_out: Outbox)
    requires retirement::domain(before), retirement::accepted(before), !before.fail_fast,
        fast.fail_fast, port_failfast_admission::same_world(before, fast),
        before.file_sets@.len() == 0, before.next_question < u64::MAX,
        before.nodes@[0].phase == Phase::Making, node_headroom(before.nodes@[0]), initial.len == 0,
        port_review_reply::active(before.nodes@[0], generation),
        footprint::valid_footprint(before, files),
        port_replies_maker_admission::inline_input(before, Reply::MakerNeedsFiles { files, fits }),
        port_review_reply::reply_post(before, off, 0, generation,
            Reply::MakerNeedsFiles { files, fits }, initial, off_out),
        port_review_reply::reply_post(fast, on,
            0, generation, Reply::MakerNeedsFiles { files, fits }, initial, on_out),
        !(fits && !footprint::footprint_empty(files)),
    ensures port_failfast_replies::diverted(before, off, on, Reply::MakerNeedsFiles { files, fits }, on_out),
{
    reveal(port_review_reply::accepted_reply);
    footprint::storage_frame(before, fast, files, files);
    room_side(before, off, generation, files, fits, initial, off_out);
    room_side(fast, on, generation, files, fits, initial, on_out);
    port_failfast_replies::retry_pair(before, fast, off, on, 1, ReviewStop::MakerNeedsFiles,
        initial, off_out, on_out);
}

/// Q54 MakerNeedsFiles: funded unions agree, unfunded unions ask for credit,
/// and non-fitting/empty inputs instantiate the same parametric room diversion.
pub(super) proof fn compare(before: World, fast: World, off: World, on: World, generation: u64,
    files: Footprint, fits: bool, initial: Outbox, off_out: Outbox, on_out: Outbox)
    requires retirement::domain(before), retirement::accepted(before), !before.fail_fast,
        fast.fail_fast, port_failfast_admission::same_world(before, fast),
        before.file_sets@.len() == 0, before.next_question < u64::MAX,
        before.nodes@[0].phase == Phase::Making, node_headroom(before.nodes@[0]), initial.len == 0,
        port_review_reply::active(before.nodes@[0], generation),
        footprint::valid_footprint(before, files),
        port_replies_maker_admission::inline_input(before, Reply::MakerNeedsFiles { files, fits }),
        port_review_reply::reply_post(before, off, 0, generation,
            Reply::MakerNeedsFiles { files, fits }, initial, off_out),
        port_review_reply::reply_post(fast, on,
            0, generation, Reply::MakerNeedsFiles { files, fits }, initial, on_out),
    ensures port_failfast_replies::diverted(before, off, on, Reply::MakerNeedsFiles { files, fits }, on_out),
{
    if fits && !footprint::footprint_empty(files) {
        if port_review_retry::funded(port_review_reply::ended(before.nodes@[0])) {
            joined_funded(before, fast, off, on, generation, files, fits, initial, off_out, on_out);
        } else {
            joined_unfunded(before, fast, off, on, generation, files, fits, initial, off_out, on_out);
        }
    } else {
        exhausted(before, fast, off, on, generation, files, fits, initial, off_out, on_out);
    }
}
}
