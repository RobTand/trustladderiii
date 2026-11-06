//! #806: real apply and the complete production checker for all four maker replies.
use super::retirement::{accepted, bounded, domain, notices_only};
use super::*;

verus! {
/// #806: expose the actual reply result to original-domain executable covers.
pub(super) open spec fn actual(before: World, after: World, generation: u64, reply: Reply,
    out_after: Outbox) -> bool {
    exists|out_before: Outbox| empty_outbox(out_before)
        && port_review_reply::reply_post(before, after, 0, generation, reply, out_before, out_after)
}

/// Q54 / #806: retain check_apply's complete arbitrary-generation funding assertion.
pub(super) open spec fn funding(before: World, after: World, generation: u64, reply: Reply) -> bool {
    if let Reply::MakerNeedsFiles { files, fits } = reply {
        let n = before.nodes@[0]; let a = after.nodes@[0];
        before.fail_fast && n.phase == Phase::Making && current_item(n) == Some(Item::Await)
            && n.run == Some(generation) && footprint::valid_footprint(before, files)
            && fits && !footprint::footprint_empty(files) && !port_review_retry::funded(n)
            ==> a.phase == Phase::AwaitingRob && a.question.is_some()
                && a.question.unwrap().kind == QKind::OutOfCredit
                && a.question.unwrap().cont == Cont::Ready(Resume::Making)
                && a.review_stop.is_none() && a.credit == n.credit
    } else { true }
}
}

kernel_fn! {
/// #806 maker reply rows: one real apply, complete checker before and after,
/// every shared node/output/Q54 assertion and the exact accepted or inactive frame.
/// Original four-file inputs are strengthened to arbitrary inline bits; there
/// is no settledness, small-head, pool/width or original ledger upper bound premise.
pub(super) fn verify_apply(mut w: World, generation: u64, reply: Reply)
    -> (result: (World, Outbox)) [
    requires domain(w), accepted(w), bounded(w), storage_ordered(w),
        port_replies_maker::selected(reply), port_replies_maker_admission::inline_input(w, reply),
    ensures domain(result.0), accepted(result.0),
        port_replies_safety::shared(w, result.0, reply), notices_only(result.1),
        port_replies_admission::table_frame(w, result.0),
        actual(w, result.0, generation, reply, result.1), funding(w, result.0, generation, reply),
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
        port_replies_maker_admission::reply_safe(before@, w, generation, reply, before_out@, out);
        port_replies_safety::checker_headroom(before@, w);
        reveal(storage_ordered); reveal(footprint::ordered_table);
        if let Reply::MakerNeedsFiles { files, fits } = reply {
            let n = before@.nodes@[0];
            if before@.fail_fast && n.phase == Phase::Making && current_item(n) == Some(Item::Await)
                && n.run == Some(generation) && footprint::valid_footprint(before@, files)
                && fits && !footprint::footprint_empty(files) && !port_review_retry::funded(n) {
                port_replies_maker_frames::funding(before@, w, generation, files, fits, before_out@, out);
            }
        }
        assert(actual(before@, w, generation, reply, out));
    }
    let checked_after = w.check();
    assert!(checked_after.is_ok());
    (w, out)
}
}
