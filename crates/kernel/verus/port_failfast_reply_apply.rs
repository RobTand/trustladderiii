//! Q54 / #440 / #806: the six reply diversions through paired real apply calls.
use super::*;

verus! {
/// Q54: compose both actual replies through the same parametric diversion lemma.
pub(super) proof fn compare(before: World, off: World, on: World, generation: u64, reply: Reply,
    initial: Outbox, off_out: Outbox, on_out: Outbox)
    requires retirement::domain(before), retirement::accepted(before), retirement::bounded(before),
        !before.fail_fast, before.file_sets@.len() == 0,
        port_failfast_replies::selected(before.nodes@[0], reply), initial.len == 0,
        port_replies_maker_admission::inline_input(before, reply),
        port_review_reply::reply_post(before, off, 0, generation, reply, initial, off_out),
        port_review_reply::reply_post(World { fail_fast: true, ..before }, on,
            0, generation, reply, initial, on_out),
    ensures port_failfast_replies::diverted(before, off, on, reply, on_out),
{
    reveal(retirement::bounded);
    if !port_review_reply::active(before.nodes@[0], generation)
        || !port_review_reply::accepted_reply(before, before.nodes@[0], reply) {
        port_failfast_admission::inactive_pair(before, off, on, generation, reply, initial, off_out, on_out);
    } else if let Reply::MakerNeedsFiles { files, fits } = reply {
        reveal(port_review_reply::accepted_reply);
        port_failfast_files::compare(before, off, on, generation, files, fits, initial, off_out, on_out);
    } else {
        port_failfast_replies::ordinary(before, off, on, generation, reply, initial, off_out, on_out);
    }
}

/// Q54 / #832-A: lift each published safety frame to the current shared stop rule.
pub(super) proof fn safe(before: World, after: World, generation: u64, reply: Reply,
    initial: Outbox, out: Outbox)
    requires retirement::domain(before), retirement::accepted(before), retirement::bounded(before),
        before.fail_fast, before.file_sets@.len() == 0,
        port_failfast_replies::selected(before.nodes@[0], reply), retirement::notices_only(initial),
        port_replies_maker_admission::inline_input(before, reply),
        port_review_reply::reply_post(before, after, 0, generation, reply, initial, out),
    ensures retirement::domain(after), retirement::accepted(after), retirement::notices_only(out),
        forall|i: int| 0 <= i < 2 ==> port_failfast_safety::node_post(before.nodes@[i], after.nodes@[i]),
{
    if port_replies_maker::selected(reply) {
        port_replies_maker_admission::reply_safe(before, after, generation, reply, initial, out);
        reveal(port_replies_frames::node_change);
    } else if let Reply::ChecksDone { report, pass, in_footprint } = reply {
        port_failfast_checks::world(before, after, generation, report, pass, in_footprint, initial, out);
    } else {
        reveal(retirement::bounded);
        port_review_world::world_rules(before, after, generation, reply, initial, out);
        reveal(port_review_safety::node_post);
    }
    reveal(port_review_reply::reply_post);
    assert forall|i: int| 0 <= i < 2 implies
        port_failfast_safety::node_post(before.nodes@[i], after.nodes@[i]) by {
        assert(i == 0 || i == 1);
    }
}
}

kernel_fn! {
/// Q54 / #440 six reply harnesses: run real apply under both flags for all
/// generations and payloads, preserving every original paired assertion.
/// Settledness and the original small file/head/gate/credit bounds are omitted.
pub(super) fn reply(mut off: World, generation: u64, reply: Reply)
    -> (result: ((World, Outbox), (World, Outbox))) [
    requires retirement::domain(off), retirement::accepted(off), retirement::bounded(off), storage_ordered(off),
        !off.fail_fast, off.file_sets@.len() == 0,
        port_failfast_replies::selected(off.nodes@[0], reply),
        port_replies_maker_admission::inline_input(off, reply),
    ensures !result.0.0.fail_fast, result.1.0.fail_fast,
        retirement::domain(result.1.0), retirement::accepted(result.1.0), retirement::notices_only(result.1.1),
        port_failfast_replies::diverted(off, result.0.0, result.1.0, reply, result.1.1),
        forall|i: int| 0 <= i < 2 ==> port_failfast_safety::node_post(off.nodes@[i], result.1.0.nodes@[i]),
        !port_review_reply::active(off.nodes@[0], generation) ==>
            port_failfast_admission::same_world(result.0.0, result.1.0),
        port_review_reply::reply_post(off, result.0.0, 0, generation, reply,
            Outbox { items: [None; OUTBOX], len: 0 }, result.0.1),
        port_review_reply::reply_post(World { fail_fast: true, ..off }, result.1.0, 0, generation, reply,
            Outbox { items: [None; OUTBOX], len: 0 }, result.1.1),
] proof [reveal(retirement_domain); reveal(retirement_accepted); reveal(empty_outbox);
    reveal(retirement::notices_only);] {
    kernel_snapshot! { before = off }
    let mut on = port_failfast_witness::fast_copy(&off);
    kernel_snapshot! { fast = on }
    kernel_proof! {
        port_failfast_admission::stale_world(off, on);
        port_replies_safety::apply_scope(off); port_replies_safety::apply_scope(on);
        if let Reply::MakerNeedsFiles { files, .. } = reply {
            footprint::storage_frame(off, on, files, files);
        }
    }
    let checked_before = off.check();
    assert!(checked_before.is_ok());
    let mut out_off = Outbox::new();
    let mut out_on = Outbox::new();
    kernel_snapshot! { initial = out_off }
    kernel_snapshot! { initial_on = out_on }
    apply(&mut off, Event::Reply { node: NodeId(0), generation, reply }, &mut out_off);
    apply(&mut on, Event::Reply { node: NodeId(0), generation, reply }, &mut out_on);
    kernel_proof! {
        compare(before@, off, on, generation, reply, initial@, out_off, out_on);
        safe(fast@, on, generation, reply, initial_on@, out_on);
        if !port_review_reply::active(before@.nodes@[0], generation) {
            port_failfast_admission::inactive_pair(before@, off, on, generation,
                reply, initial@, out_off, out_on);
        }
        reveal(port_review_reply::reply_post);
        reveal(port_review_reply::reply_result); reveal(port_review_reply::transition_post);
        reveal(port_replies_contracts::transition_post); reveal(port_replies_maker::transition_post);
        reveal(port_replies_maker::files_post); reveal(port_replies_maker::work_result);
        reveal(port_review_reply::transition); reveal(port_review_retry::round);
        reveal(port_review_retry::rung); reveal(port_review_retry::room); reveal(port_review_retry::rechecked);
        reveal(port_review_retry::failed_run); reveal(retirement::incident); reveal(retirement::stopped);
        port_replies_safety::checker_headroom(fast@, on);
    }
    let checked_after = on.check();
    assert!(checked_after.is_ok());
    ((off, out_off), (on, out_on))
}
}
