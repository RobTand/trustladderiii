//! Q54 / #440 / #806: compare actual maker-file unions, credit questions and retries.
use super::*;

verus! {
/// Original-domain empty storage implies inline handles for every valid input.
pub(super) proof fn inline_input(w: World, reply: Reply)
    requires retirement::domain(w), retirement::accepted(w), w.file_sets@.len() == 0,
    ensures port_replies_maker_admission::inline_input(w, reply),
{
    reveal(retirement::accepted); reveal(retirement::node_rules);
    reveal(footprint::valid_footprint); reveal(footprint::inline_footprint);
}

/// Q54 MakerNeedsFiles: funded unions agree, unfunded unions ask for credit,
/// and non-fitting/empty inputs instantiate the same parametric room diversion.
pub(super) proof fn compare(before: World, off: World, on: World, generation: u64,
    files: Footprint, fits: bool, initial: Outbox, off_out: Outbox, on_out: Outbox)
    requires retirement::domain(before), retirement::accepted(before), !before.fail_fast,
        before.file_sets@.len() == 0, before.next_question < u64::MAX,
        before.nodes@[0].phase == Phase::Making, node_headroom(before.nodes@[0]), initial.len == 0,
        port_review_reply::active(before.nodes@[0], generation),
        footprint::valid_footprint(before, files),
        port_review_reply::reply_post(before, off, 0, generation,
            Reply::MakerNeedsFiles { files, fits }, initial, off_out),
        port_review_reply::reply_post(World { fail_fast: true, ..before }, on,
            0, generation, Reply::MakerNeedsFiles { files, fits }, initial, on_out),
    ensures port_failfast_replies::diverted(before, off, on, Reply::MakerNeedsFiles { files, fits }, on_out),
{
    let reply = Reply::MakerNeedsFiles { files, fits };
    let source = port_review_reply::ended(before.nodes@[0]);
    reveal(port_review_reply::accepted_reply);
    port_failfast_replies::admitted(before, off, generation, reply, initial, off_out);
    port_failfast_replies::admitted(World { fail_fast: true, ..before }, on,
        generation, reply, initial, on_out);
    inline_input(before, reply);
    reveal(port_replies_maker::transition_post); reveal(port_replies_maker::files_post);
    reveal(port_review_reply::reply_post); reveal(port_replies_maker::union_post);
    reveal(retirement::accepted); reveal(retirement::node_rules);
    if fits && !footprint::footprint_empty(files) {
        let left = choose|bits: u64| source.footprint == footprint::inline_footprint(bits);
        let right = choose|bits: u64| files == footprint::inline_footprint(bits);
        assert(on.nodes@[0].footprint == footprint::inline_footprint(left | right));
        if port_review_retry::funded(source) {
            assert(off.nodes@[0].footprint == on.nodes@[0].footprint);
            assert(off.nodes@ =~= on.nodes@);
        } else {
            port_failfast_pair::funding(source, on.nodes@[0].footprint, before.next_question);
            reveal(port_review_retry::room);
        }
    } else {
        port_failfast_pair_frame::compare(before, off, on, source, 1, ReviewStop::MakerNeedsFiles, on_out);
    }
}
}
