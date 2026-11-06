//! #860: complete checker, check_nodes/check_outputs and Q54 over the real apply.
use super::port_t860_contracts::*;
use super::*;

kernel_fn! {
/// Auditing / #860: one real apply, all original phases and arbitrary generations.
/// Two nodes/two gates retain the original domain; no settledness is required.
/// Ledger bounds are weaker than Kani's; there is no unwind or world unrolling.
pub(super) fn verify_apply(mut w: World, generation: u64) -> (result: (World, Outbox)) [
    requires retirement::domain(w), retirement::accepted(w), retirement::bounded(w), storage_ordered(w),
    ensures retirement::domain(result.0), retirement::accepted(result.0),
        result.0.nodes@ == w.nodes@.update(0, reply_node(w.nodes@[0], generation)),
        result.0.next_question == w.next_question,
        result.0.nodes@[0].credit == w.nodes@[0].credit,
        retirement::notices_only(result.1),
        (w.nodes@[0].phase == Phase::Auditing ==> empty_outbox(result.1)),
        forall|i: int| 0 <= i < 2 ==> node_change(w.nodes@[i], result.0.nodes@[i], w.fail_fast,
            Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees }, i),
] proof [reveal(retirement_domain); reveal(retirement_accepted); reveal(empty_outbox);] {
    kernel_snapshot! { before = w }
    kernel_proof! { port_t860_scope::apply_scope(w); }
    let checked_before = w.check();
    assert!(checked_before.is_ok());
    let mut out = Outbox::new();
    kernel_snapshot! { before_out = out }
    kernel_proof! { reveal(retirement::notices_only); }
    apply(&mut w, Event::Reply { node: NodeId(0), generation, reply: Reply::AuditDisagrees }, &mut out);
    kernel_proof! {
        reveal(apply_post);
        port_t860_frames::world(before@, w, generation, before_out@, out);
        port_t860_scope::checker_headroom(before@, w);
        reveal(apply_post); reveal(reply_post); reveal(reply_node); reveal(failed_node); reveal(audit_node); reveal(ended);
        reveal(retirement::stopped); reveal(retirement::incident);
        reveal(storage_ordered); reveal(footprint::ordered_table);
    }
    let checked_after = w.check();
    assert!(checked_after.is_ok());
    (w, out)
}
}
