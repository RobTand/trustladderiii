//! #806: source-connected acknowledgement application and output/refusal assertions.
use super::port_acks_specs::*;
use super::retirement::{accepted, bounded, domain};
use super::*;

verus! {
/// State-table Ack outputs / #806: original check_effect assertions, with Ack's pause exemption.
#[verifier::opaque]
pub(super) open spec fn output_rules(b: World, a: World, out: Outbox) -> bool {
    forall|k: int| 0 <= k < OUTBOX ==> match #[trigger] out.items@[k] {
        Some(Out::Effect { node, seq, kind, head }) => node.0 < a.nodes@.len()
            && seq < a.nodes@[node.0 as int].next_seq
            && (node.0 < b.nodes@.len() ==> seq >= b.nodes@[node.0 as int].next_seq)
            && (kind == EffectKind::Enqueue ==> match head {
                Some(h) => a.nodes@[node.0 as int].checked == Some(h)
                    && a.nodes@[node.0 as int].approved == Some(h), None => false,
            }),
        _ => true,
    }
}

/// State-table retry emission / #806: exact fresh key, valid target and approved enqueue head.
pub(super) proof fn ack_output_rules(b: World, a: World, seq: u64, ack: Ack, ob: Outbox, oa: Outbox)
    requires domain(b), accepted(b), bounded(b), ack_post(b, a, 0, seq, ack, ob, oa), empty_outbox(ob),
    ensures output_rules(b, a, oa),
{
    reveal(ack_post); reveal(world_frame); reveal(ack_output); reveal(retry_output); reveal(ack_emissions); reveal(appended); reveal(retirement::notices_only);
    reveal(output_rules); reveal(empty_outbox); reveal(accepted); reveal(retirement::node_rules);
    reveal(ack_node); reveal(retry_node); reveal(failed_delivery); reveal(emitted_node);
    assert forall|k: int| 0 <= k < OUTBOX implies match #[trigger] oa.items@[k] {
        Some(Out::Effect { node, seq: key, kind, head }) => node.0 < a.nodes@.len()
            && key < a.nodes@[node.0 as int].next_seq
            && (node.0 < b.nodes@.len() ==> key >= b.nodes@[node.0 as int].next_seq)
            && (kind == EffectKind::Enqueue ==> match head {
                Some(h) => a.nodes@[node.0 as int].checked == Some(h)
                    && a.nodes@[node.0 as int].approved == Some(h), None => false,
            }), _ => true,
    } by {}
}

/// #806: output relations depend on buffer contents and length, not array identity.
pub(super) proof fn ack_output_context(n: Node, q: u64, i: usize, seq: u64, ack: Ack, ob: Outbox, equivalent: Outbox, oa: Outbox)
    requires ob.len == equivalent.len, ob.items@ == equivalent.items@,
        ack_output(n, q, i, seq, ack, ob, oa),
    ensures ack_output(n, q, i, seq, ack, equivalent, oa),
{
    reveal(ack_output); reveal(retry_output); reveal(appended); reveal(retirement::notices_only);
}

/// #629: non-OpenPr body refusal has the identical node, counter and complete output result.
pub(super) proof fn refusal_is_permanent(n: Node, q: u64, i: usize, seq: u64)
    requires !publication(n, seq, Ack::BodyPolicyRefused),
    ensures ack_node(n, q, seq, Ack::BodyPolicyRefused) == ack_node(n, q, seq, Ack::Permanent),
        asks(n, seq, Ack::BodyPolicyRefused) == asks(n, seq, Ack::Permanent),
        ack_emissions(n, q, i, seq, Ack::BodyPolicyRefused) == ack_emissions(n, q, i, seq, Ack::Permanent),
{
    reveal(ack_node); reveal(ack_emissions);
}

/// #629: an OpenPr body refusal emits no question or incident at either fail_fast value.
pub(super) proof fn refusal_output(n: Node, q: u64, seq: u64)
    requires publication(n, seq, Ack::BodyPolicyRefused),
    ensures ack_emissions(n, q, 0, seq, Ack::BodyPolicyRefused).len() == 0,
        ack_node(n, q, seq, Ack::BodyPolicyRefused).review_stop == Some(ReviewStop::PublicationBody),
{
    reveal(ack_emissions); reveal(ack_node); reveal(retirement::stopped);
}
}

kernel_fn! {
/// State-table Ack rows / #806: one real apply and the complete production checker.
/// Original bounded two-node worlds are included; settledness and small IDs are unnecessary.
pub(super) fn verify_ack_apply(mut w: World, seq: u64, ack: Ack) -> (result: (World, Outbox)) [
    requires domain(w), accepted(w), bounded(w), storage_ordered(w),
    ensures domain(result.0), accepted(result.0),
        world_frame(w, result.0, 0),
        result.0.nodes@[0] == ack_node(w.nodes@[0], w.next_question, seq, ack),
        result.0.next_question as int == w.next_question as int + (if asks(w.nodes@[0], seq, ack) { 1int } else { 0int }),
        ack_output(w.nodes@[0], w.next_question, 0, seq, ack, port_acks_specs::empty(), result.1),
        output_rules(w, result.0, result.1),
        forall|j: int| 0 <= j < 2 ==> node_transition(w.nodes@[j], result.0.nodes@[j])
            && q54(w.nodes@[j], result.0.nodes@[j], w.fail_fast, j == 0 && publication(w.nodes@[0], seq, ack)),
] proof [reveal(world_headroom); reveal_with_fuel(open_prefix, 3); reveal(bounded);
    reveal(empty_outbox); reveal(retirement_domain); reveal(retirement_accepted);
    reveal(accepted); reveal(ack_post); reveal(appended); reveal(port_acks_specs::empty);] {
    kernel_snapshot! { original = w }
    kernel_proof! {
        open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
        dependencies::indexable_definition(w);
    }
    let checked = w.check();
    assert!(checked.is_ok());
    kernel_proof! { world_checked_definition(w); reveal(storage_ordered); }
    let mut out = Outbox::new();
    kernel_snapshot! { original_out = out }
    apply(&mut w, Event::Ack { node: NodeId(0), seq, ack }, &mut out);
    kernel_proof! {
        port_acks_safety::ack_world(original@, w, seq, ack, original_out@, out);
        ack_output_rules(original@, w, seq, ack, original_out@, out);
        assert(original_out@.items@ =~= port_acks_specs::empty().items@);
        ack_output_context(original@.nodes@[0], original@.next_question, 0, seq, ack,
            original_out@, port_acks_specs::empty(), out);
    }
    let checked_after = w.check();
    assert!(checked_after.is_ok());
    (w, out)
}
}

verus! {
/// #629: executable World equality compares these fields and complete vector contents.
pub(super) open spec fn same_world(a: World, b: World) -> bool {
    a.nodes@ == b.nodes@ && a.gates@ == b.gates@ && a.file_sets@ == b.file_sets@
        && a.dependencies@ == b.dependencies@ && a.pool_capacity == b.pool_capacity
        && a.next_ticket == b.next_ticket && a.next_question == b.next_question && a.fail_fast == b.fail_fast
}
}

kernel_fn! {
/// #629 / #806: run the actual refusal and Permanent paths from equal original-domain worlds.
/// Non-OpenPr results agree in every world field and every outbox slot, including stale/cancelled keys.
pub(super) fn verify_refusal_comparison(w: World, seq: u64) -> (result: ((World, Outbox), (World, Outbox))) [
    requires domain(w), accepted(w), bounded(w), storage_ordered(w), w.file_sets@.len() == 0,
    ensures publication(w.nodes@[0], seq, Ack::BodyPolicyRefused) ==> result.0.1.len == 0,
        !publication(w.nodes@[0], seq, Ack::BodyPolicyRefused) ==>
            same_world(result.0.0, result.1.0) && result.0.1.len == result.1.1.len
                && result.0.1.items@ == result.1.1.items@,
] proof [reveal(ack_output); reveal(world_frame); reveal(same_world); reveal(appended); reveal(port_acks_specs::empty);] {
    kernel_snapshot! { original = w }
    let permanent_input = snapshot_world(&w);
    kernel_proof! {
        assert(permanent_input.file_sets@ =~= w.file_sets@);
        reveal(accepted); reveal(bounded); reveal(storage_ordered); reveal(footprint::ordered_table);
        assert(storage_ordered(permanent_input));
        assert(domain(permanent_input));
        retirement::node_frame(w, permanent_input, w.nodes@[0]);
        retirement::node_frame(w, permanent_input, w.nodes@[1]);
        footprint::storage_frame(w, permanent_input, w.nodes@[0].footprint, w.nodes@[1].footprint);
    }
    let refused = verify_ack_apply(w, seq, Ack::BodyPolicyRefused);
    let permanent = verify_ack_apply(permanent_input, seq, Ack::Permanent);
    kernel_proof! {
        if publication(original@.nodes@[0], seq, Ack::BodyPolicyRefused) {
            refusal_output(original@.nodes@[0], original@.next_question, seq);
        } else {
            refusal_is_permanent(original@.nodes@[0], original@.next_question, 0, seq);
            assert(refused.0.nodes@ =~= permanent.0.nodes@);
            assert(refused.1.items@ =~= permanent.1.items@);
        }
    }
    (refused, permanent)
}
}
