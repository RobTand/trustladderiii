//! #806: verbatim pre-extraction bodies retained for equivalence testing.
use super::*;

#[rustfmt::skip]
pub(super) fn on_ack(w: &mut World, i: usize, seq: u64, ack: Ack, out: &mut Outbox) {
    kernel_snapshot! { ack_before = *w }
    kernel_snapshot! { ack_out_before = *out }
    let Some(e) = w.nodes[i].effect else {
        kernel_proof! {
            port_acks_frames::inactive_ack(ack_before@.nodes@[i as int], w.nodes@[i as int],
                w.next_question, i, seq, ack, ack_out_before@, *out);
        }
        return
    };
    if e.seq != seq {
        kernel_proof! {
            port_acks_frames::inactive_ack(ack_before@.nodes@[i as int], w.nodes@[i as int],
                w.next_question, i, seq, ack, ack_out_before@, *out);
        }
        return;
    }
    w.nodes[i].effect = None;
    kernel_snapshot! { ack_cleared = w.nodes@[i as int] }
    kernel_proof! {
        assert(port_acks_specs::cleared_post(ack_before@.nodes@[i as int], ack_cleared@)) by {
            reveal(port_acks_specs::cleared_post);
        }
    }
    kernel_proof! { port_acks_frames::ack_clear_input(ack_before@.nodes@[i as int], ack_cleared@, e); }
    if e.cancelling {
        kernel_proof! {
            port_acks_frames::cancelled_ack_progress(ack_before@, ack_before@.nodes@[i as int], w.nodes@[i as int], e);
            port_acks_frames::cancelled_ack(ack_before@.nodes@[i as int], w.nodes@[i as int],
                w.next_question, i, seq, ack, ack_out_before@, e);
        }
        return;
    }
    match ack {
        Ack::Confirmed => {
            confirmed(&mut w.nodes[i], e.kind);
            kernel_proof! { port_acks_frames::confirmed_ack_progress(ack_before@,
                ack_before@.nodes@[i as int], ack_cleared@, w.nodes@[i as int], e);
                port_acks_frames::confirmed_live(ack_before@.nodes@[i as int], ack_cleared@,
                    w.nodes@[i as int], ack_before@.next_question, i, seq, ack_out_before@, e); }
        },
        Ack::Retryable => { delivery_failed(w, i, e.kind, out); kernel_proof! { port_acks_frames::retry_live(ack_before@.nodes@[i as int], ack_cleared@,
                    w.nodes@[i as int], ack_before@.next_question, i, seq, ack_out_before@, *out, e); } },
        Ack::Permanent => { effect_failed(w, i, e.kind, out); kernel_proof! { port_acks_frames::permanent_live(ack_before@.nodes@[i as int], ack_cleared@,
                    w.nodes@[i as int], ack_before@.next_question, w.next_question, i, seq, ack, ack_out_before@, *out, e); } },
        Ack::BodyPolicyRefused if e.kind == EffectKind::OpenPr => {
            stop_for_review(&mut w.nodes[i], ReviewStop::PublicationBody);
            kernel_proof! { port_acks_frames::refused_live(ack_before@.nodes@[i as int], ack_cleared@,
                    w.nodes@[i as int], ack_before@.next_question, i, seq, ack_out_before@, e); }
        }
        Ack::BodyPolicyRefused => { effect_failed(w, i, e.kind, out); kernel_proof! { port_acks_frames::permanent_live(ack_before@.nodes@[i as int], ack_cleared@,
                    w.nodes@[i as int], ack_before@.next_question, w.next_question, i, seq, ack, ack_out_before@, *out, e); } },
    }
}

#[rustfmt::skip]
pub(super) fn confirmed_agenda(m: &mut Node, kind: EffectKind) {
    kernel_snapshot! { before_agenda = *m }
    kernel_proof! { port_acks_frames::agenda_input(*m); }
    if !kind.starts_operation() {
        if let Some(Item::Do(k) | Item::Teardown(k)) = m.current() {
            if k == kind {
                if kind == EffectKind::PrepareWorktree && !m.adopted {
                    if let Some(head) = m.proposed {
                        m.adopted = true;
                        m.head = Some(head);
                        m.checked = None;
                        kernel_snapshot! { adopted_before_phase = *m }
                        kernel_proof! {
                            assert(port_acks_specs::adoption_post(before_agenda@, adopted_before_phase@)) by {
                                reveal(port_acks_specs::adoption_post);
                            }
                        }
                        set_phase(m, Phase::Checking);
                        kernel_proof! { port_acks_frames::agenda_adoption_composed(before_agenda@, adopted_before_phase@, *m, kind);
        port_acks_frames::agenda_contract(before_agenda@, *m, kind);
        port_acks_frames::adoption_fields(before_agenda@, *m, kind, false); }
                        return;
                    }
                }
                m.pos += 1;
                kernel_proof! {
                    assert(port_acks_specs::handled_increment(before_agenda@, *m)) by {
                        reveal(port_acks_specs::handled_increment);
                    }
                    port_acks_frames::agenda_incremented(before_agenda@, *m, kind);
                }
            }
        }
    }
    kernel_proof! {
            if matches!(kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
                || !(current_item(before_agenda@) == Some(Item::Do(kind)) || current_item(before_agenda@) == Some(Item::Teardown(kind))) {
                port_acks_frames::agenda_stutter(before_agenda@, kind);
            }
            port_acks_frames::agenda_contract(before_agenda@, *m, kind);
        port_acks_frames::adoption_fields(before_agenda@, *m, kind, false);
    }
}

#[rustfmt::skip]
pub(super) fn apply_ack_event(w: &mut World, node: NodeId, seq: u64, ack: Ack, out: &mut Outbox) {
            if let Some(i) = w.index(node) {
                on_ack(w, i, seq, ack, out);
                kernel_proof! {
                    port_acks_frames::finish_ack(before_apply@, *w, i, seq, ack, before_apply_out@, *out);
                    if progress_input(before_apply@, event) {
                        assert(w.nodes@ =~= before_apply@.nodes@.update(i as int, w.nodes@[i as int]));
                        progress::lift_node_progress(before_apply@, *w, i as int);
                        dependencies::indexability_frame(before_apply@, *w);
                    }
                }
            }
}
