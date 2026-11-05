//! Executable acknowledgements transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
/// Acknowledgement rows / #771: stale delivery keys stutter; retries stay bounded.
pub(super) fn on_ack(w: &mut World, i: usize, seq: u64, ack: Ack,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures (old(w).nodes@[i as int].phase is Closed && old(w).nodes@[i as int].effect.is_none()
            ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) && stopping_effect_fits(old(w).nodes@[i as int])
            ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq as int <= old(w).nodes@[i as int].next_seq as int + 1,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 2,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    let Some(e) = w.nodes[i].effect else { return };
    if e.seq != seq {
        return;
    }
    w.nodes[i].effect = None;
    if e.cancelling {
        return;
    }
    match ack {
        Ack::Confirmed => confirmed(&mut w.nodes[i], e.kind),
        Ack::Retryable => delivery_failed(w, i, e.kind, out),
        Ack::Permanent => effect_failed(w, i, e.kind, out),
        Ack::BodyPolicyRefused if e.kind == EffectKind::OpenPr => {
            stop_for_review(&mut w.nodes[i], ReviewStop::PublicationBody);
        }
        Ack::BodyPolicyRefused => effect_failed(w, i, e.kind, out),
    }
}
}

kernel_fn! {
/// Confirmed-effect rows / #771: at most one adoption generation is allocated.
// The pinned Verus frontend requires nested if-let instead of let chains.
#[allow(clippy::collapsible_if)]
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn confirmed(m: &mut Node, kind: EffectKind) -> (result: ()) [
    requires old(m).generation < u64::MAX,
    ensures final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m))
            && (old(m).phase is Stopping ==> current_item(*old(m)) == Some(Item::Teardown(kind)))
            ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).generation >= old(m).generation,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).question == old(m).question,
        final(m).fx_failures == 0, final(m).fx_incidents == 0,
        final(m).last_cmd == old(m).last_cmd,
] {
    m.fx_failures = 0;
    m.fx_incidents = 0;
    match kind {
        EffectKind::StopRun => {
            m.run = None;
            m.pool = false;
        }
        EffectKind::WithdrawJob => m.job = None,
        EffectKind::ClosePr => m.pr_open = false,
        EffectKind::RemoveWorktree => m.worktree = false,
        _ => {}
    }
    if !kind.starts_operation() {
        if let Some(Item::Do(k) | Item::Teardown(k)) = m.current() {
            if k == kind {
                if kind == EffectKind::PrepareWorktree && !m.adopted {
                    if let Some(head) = m.proposed {
                        m.adopted = true;
                        m.head = Some(head);
                        m.checked = None;
                        set_phase(m, Phase::Checking);
                        return;
                    }
                }
                m.pos += 1;
            }
        }
    }
}
}

kernel_fn! {
/// Failed-delivery rows / #771: at most two generations or one retried effect.
pub(super) fn delivery_failed(w: &mut World, i: usize, kind: EffectKind,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        old(w).nodes@[i as int].generation as int + 2 <= u64::MAX as int,
        old(w).nodes@[i as int].next_seq < u64::MAX,
        old(w).nodes@[i as int].fx_failures <= 1,
        old(w).nodes@[i as int].fx_incidents <= 2,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq as int <= old(w).nodes@[i as int].next_seq as int + 1,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).nodes@[i as int].question == old(w).nodes@[i as int].question,
        final(w).nodes@[i as int].fx_failures <= 1,
        final(w).nodes@[i as int].fx_incidents <= 2,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
        kernel_snapshot! { before_budget = *w }
        kernel_snapshot! { budget_out = out.len }

    let m = &mut w.nodes[i];
    m.fx_failures += 1;
    if m.fx_failures < 2 {
        emit(w, i, kind, out);
                kernel_proof! {
            assert(w.nodes@.len() == before_budget@.nodes@.len());
            assert forall|j: int| 0 <= j < before_budget@.nodes@.len() && j != i implies
                w.nodes@[j] == before_budget@.nodes@[j] by {}
            assert(w.nodes@[i as int].generation as int <= before_budget@.nodes@[i as int].generation as int + 2);
            assert(w.nodes@[i as int].next_seq as int <= before_budget@.nodes@[i as int].next_seq as int + 1);
            assert(w.nodes@[i as int].credit.start == before_budget@.nodes@[i as int].credit.start);
            assert(w.nodes@[i as int].credit.earned == before_budget@.nodes@[i as int].credit.earned);
            assert(w.nodes@[i as int].credit.added == before_budget@.nodes@[i as int].credit.added);
            assert(w.nodes@[i as int].credit.spent == before_budget@.nodes@[i as int].credit.spent);
            assert(w.nodes@[i as int].last_cmd == before_budget@.nodes@[i as int].last_cmd);
            assert(w.next_ticket == before_budget@.next_ticket);
            assert(w.next_question == before_budget@.next_question);
            assert(out.len as int <= budget_out@ as int + 1);
        }
        return;
    }
    m.fx_failures = 0;
    m.fx_incidents = (m.fx_incidents + 1).min(2);
    let needs_rob = m.fx_incidents >= 2;
    if kind.starts_operation() {
        abandon_operation(m);
        m.pos = operation_start(m.phase);
        m.generation += 1;
    }
    raise_incident(w, i, needs_rob, out);

        kernel_proof! {
            assert(w.nodes@.len() == before_budget@.nodes@.len());
            assert forall|j: int| 0 <= j < before_budget@.nodes@.len() && j != i implies
                w.nodes@[j] == before_budget@.nodes@[j] by {}
            assert(w.nodes@[i as int].generation as int <= before_budget@.nodes@[i as int].generation as int + 2);
            assert(w.nodes@[i as int].next_seq as int <= before_budget@.nodes@[i as int].next_seq as int + 1);
            assert(w.nodes@[i as int].credit.start == before_budget@.nodes@[i as int].credit.start);
            assert(w.nodes@[i as int].credit.earned == before_budget@.nodes@[i as int].credit.earned);
            assert(w.nodes@[i as int].credit.added == before_budget@.nodes@[i as int].credit.added);
            assert(w.nodes@[i as int].credit.spent == before_budget@.nodes@[i as int].credit.spent);
            assert(w.nodes@[i as int].last_cmd == before_budget@.nodes@[i as int].last_cmd);
            assert(w.next_ticket == before_budget@.next_ticket);
            assert(w.next_question == before_budget@.next_question);
            assert(out.len as int <= budget_out@ as int + 1);
        }
}
}

kernel_fn! {
/// Permanent-effect-failure rows / #771: exactly one question, no credit change.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn effect_failed(w: &mut World, i: usize, kind: EffectKind,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        old(w).nodes@[i as int].generation < u64::MAX,
        old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int == old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len == old(out).len + 2,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    notice(out, i, Notice::Incident { needs_rob: true });
    if w.nodes[i].phase.is_stopping() {
        // Cleanup stays put, keeping what it holds, until Rob retries or marks the step handled.
        let id = w.next_question;
        w.next_question += 1;
        w.nodes[i].question = Some(Question {
            id,
            kind: QKind::CleanupFailed,
            cont: Cont::Nowhere,
            cost: 0,
            clarified: false,
        });
        notice(out, i, Notice::Ask { question: id });
        return;
    }
    let m = &mut w.nodes[i];
    if kind.starts_operation() {
        abandon_operation(m);
    }
    let cont = match m.phase {
        Phase::Ingesting => Cont::Ingesting,
        Phase::Auditing => Cont::Auditing,
        Phase::Planning { must_split } => Cont::Planning { must_split },
        phase => match phase.resume() {
            Some(r) => Cont::Ready(r),
            None => Cont::Unplanned,
        },
    };
    ask_rob(w, i, QKind::EffectFailed, cont, 0, out);
}
}
