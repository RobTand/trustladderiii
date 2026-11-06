//! #806: complete checker and shared-rule proofs over the real apply boundary.
use super::port_replies_contracts::*;
use super::retirement::{domain, accepted, bounded, notices_only};
use super::*;

verus! {
/// #806: expose the admitted reply's exact node and question result to covers.
pub(super) open spec fn exact(before: World, after: World, generation: u64, reply: Reply) -> bool {
    let n = before.nodes@[0];
    !(n.phase is Stopping) && current_item(n) == Some(Item::Await)
        && (n.run == Some(generation) || n.job == Some(generation))
        && port_replies_frames::admitted(before, n, reply) ==>
        after.nodes@[0] == transitioned(ended(n), before.next_question, before.fail_fast, reply)
            && after.next_question == (before.next_question
                + if asks(n, before.fail_fast, reply) { 1u64 } else { 0u64 }) as u64
}
}

kernel_fn! {
/// #806 worker reply rows: one real apply on arbitrary consistent two-node worlds.
/// This retains the complete production checker, every shared node/output/Q54
/// assertion, and the exact frame, with weaker bounds and no settledness premise.
pub(super) fn verify_reply_apply(mut w: World, generation: u64, reply: Reply)
    -> (result: (World, Outbox)) [
    requires domain(w), accepted(w), bounded(w), storage_ordered(w), selected(reply),
    ensures domain(result.0), accepted(result.0),
        port_replies_safety::shared(w, result.0, reply),
        port_replies_admission::specific(w, result.0, generation, reply),
        notices_only(result.1), exact(w, result.0, generation, reply),
        port_replies_admission::table_frame(w, result.0),
] proof [reveal(retirement_domain); reveal(retirement_accepted); reveal(empty_outbox);] {
    kernel_snapshot! { before = w }
    kernel_proof! { port_replies_safety::apply_scope(w); }
    let checked_before = w.check();
    assert!(checked_before.is_ok());
    let mut out = Outbox::new();
    kernel_snapshot! { before_out = out }
    kernel_proof! { reveal(notices_only); }
    apply(&mut w, Event::Reply { node: NodeId(0), generation, reply }, &mut out);
    kernel_proof! {
        port_replies_admission::reply_safe(before@, w, generation, reply, before_out@, out);
        port_replies_safety::checker_headroom(before@, w);
        reveal(storage_ordered); reveal(footprint::ordered_table);
    }
    let checked_after = w.check();
    assert!(checked_after.is_ok());
    kernel_proof! {
        reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result); reveal(port_review_reply::transition_post);
        reveal(port_replies_contracts::transition_post); reveal(node_post);
        reveal(port_review_reply::accepted_reply);
    }
    (w, out)
}
}
