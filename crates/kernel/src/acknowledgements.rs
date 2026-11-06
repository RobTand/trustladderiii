//! Executable acknowledgements transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
/// Acknowledgement rows / #771: stale delivery keys stutter; retries stay bounded.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn on_ack(w: &mut World, i: usize, seq: u64, ack: Ack,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures port_acks_specs::ack_local(old(w).nodes@[i as int], final(w).nodes@[i as int],
        old(w).next_question, final(w).next_question, i, seq, ack, *old(out), *final(out)),
        (node_checked(*old(w), old(w).nodes@[i as int])
            && (!old(w).nodes@[i as int].paused || old(w).nodes@[i as int].phase is Stopping)
            && ack == Ack::Confirmed && (match old(w).nodes@[i as int].effect {
                Some(e) => e.seq == seq, None => false,
            }) ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        (old(w).nodes@[i as int].phase is Closed && old(w).nodes@[i as int].effect.is_none()
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
        // #806 adoption: neutral field guarantees consumed by the adoption owner.
        ({ let b = old(w).nodes@[i as int]; let a = final(w).nodes@[i as int];
            (b.phase == Phase::Making && b.pos == 0 && b.proposed.is_some() && ack == Ack::Confirmed && (match b.effect { Some(e) => e.kind == EffectKind::PrepareWorktree && !e.cancelling && e.seq == seq, None => false })) ==> ((a.adopted)
            && (a.head == if b.adopted { b.head } else { b.proposed })
            && (a.phase == if b.adopted { Phase::Making } else { Phase::Checking })
            && (a.pos == if b.adopted { 1u8 } else { 0u8 })
            && (a.generation as int == b.generation as int + if b.adopted { 0int } else { 1int })
            && (a.ticket == if b.adopted { b.ticket } else { None })
            && (a.checked == if b.adopted { b.checked } else { None })
            && (a.repo == b.repo)
            && (a.footprint == b.footprint)
            && (a.next_seq == b.next_seq)
            && (a.run == b.run)
            && (a.job == b.job)
            && (a.pool == b.pool)
            && (a.holds == b.holds)
            && (a.paused == b.paused)
            && (a.last_cmd == b.last_cmd)
            && (a.question == b.question)
            && (a.attempt == b.attempt)
            && (a.round == b.round)
            && (a.replanned == b.replanned)
            && (a.plan_retries == b.plan_retries)
            && (a.audit_rounds == b.audit_rounds)
            && (a.early_end == b.early_end)
            && (a.run_failures == b.run_failures)
            && (a.run_incidents == b.run_incidents)
            && (a.waiting_incident == b.waiting_incident)
            && (a.incident_needs_rob == b.incident_needs_rob)
            && (a.proposed == b.proposed)
            && (a.approved == b.approved)
            && (a.merged == b.merged)
            && (a.pr_open == b.pr_open)
            && (a.had_pr == b.had_pr)
            && (a.worktree == b.worktree)
            && (a.rechecks == b.rechecks)
            && (a.review_restarted == b.review_restarted)
            && (a.credit == b.credit)
            && (a.priority == b.priority)
            && (a.review_stop == b.review_stop)
            && (a.planning_started == b.planning_started)
            && (a.waiting_prerequisite == b.waiting_prerequisite)
            && (a.fx_failures == 0)
            && (a.fx_incidents == 0)
            && (a.effect.is_none())
            && (*final(out) == *old(out))
            && (final(w).next_question == old(w).next_question)) }),
] hide [node_checked, current_item, items, settle_shape, stopping_effect_fits] {
    kernel_snapshot! { before_ack = *w }
    kernel_snapshot! { before_ack_out = *out }
    on_ack_body(w, i, seq, ack, out);
    kernel_proof! {
        port_acks_frames::ack_node_bounds(before_ack@.nodes@[i as int], w.nodes@[i as int], before_ack@.next_question, w.next_question, i, seq, ack, before_ack_out@, *out);
        if ack == Ack::Confirmed {
            port_acks_frames::ack_adoption_fields(before_ack@.nodes@[i as int], w.nodes@[i as int], before_ack@.next_question, i, seq, before_ack_out@);
            port_acks_frames::confirmed_progress_from_local(before_ack@, before_ack@.nodes@[i as int], w.nodes@[i as int], before_ack@.next_question, i, seq, before_ack_out@);
        }
    }
}
}

kernel_fn! {
/// Confirmed-effect rows / #771: at most one adoption generation is allocated.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn confirmed(m: &mut Node, kind: EffectKind) -> (result: ()) [
    requires old(m).generation < u64::MAX,
    ensures port_acks_specs::confirmed_post(*old(m), *final(m), kind),
        (matches!(kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
            ==> distance(*final(m)) == distance(*old(m))),
        (old(m).effect.is_none() && old(m).question.is_none() && !old(m).waiting_incident
            && (!old(m).paused || old(m).phase is Stopping)
            && (current_item(*old(m)) == Some(Item::Do(kind))
                || current_item(*old(m)) == Some(Item::Teardown(kind)))
            ==> distance(*final(m)) + 2 <= distance(*old(m))),
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m))
            && (old(m).phase is Stopping ==> current_item(*old(m)) == Some(Item::Teardown(kind)))
            ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).generation >= old(m).generation,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).question == old(m).question,
        final(m).fx_failures == 0, final(m).fx_incidents == 0,
        final(m).last_cmd == old(m).last_cmd,
        // #806 adoption: neutral field guarantees consumed by the adoption owner.
        ({ let b = *old(m); let a = *final(m);
            (b.phase == Phase::Making && b.pos == 0 && b.proposed.is_some() && kind == EffectKind::PrepareWorktree) ==> ((a.adopted)
            && (a.head == if b.adopted { b.head } else { b.proposed })
            && (a.phase == if b.adopted { Phase::Making } else { Phase::Checking })
            && (a.pos == if b.adopted { 1u8 } else { 0u8 })
            && (a.generation as int == b.generation as int + if b.adopted { 0int } else { 1int })
            && (a.ticket == if b.adopted { b.ticket } else { None })
            && (a.checked == if b.adopted { b.checked } else { None })
            && (a.repo == b.repo)
            && (a.footprint == b.footprint)
            && (a.next_seq == b.next_seq)
            && (a.run == b.run)
            && (a.job == b.job)
            && (a.pool == b.pool)
            && (a.holds == b.holds)
            && (a.paused == b.paused)
            && (a.last_cmd == b.last_cmd)
            && (a.question == b.question)
            && (a.attempt == b.attempt)
            && (a.round == b.round)
            && (a.replanned == b.replanned)
            && (a.plan_retries == b.plan_retries)
            && (a.audit_rounds == b.audit_rounds)
            && (a.early_end == b.early_end)
            && (a.run_failures == b.run_failures)
            && (a.run_incidents == b.run_incidents)
            && (a.waiting_incident == b.waiting_incident)
            && (a.incident_needs_rob == b.incident_needs_rob)
            && (a.proposed == b.proposed)
            && (a.approved == b.approved)
            && (a.merged == b.merged)
            && (a.pr_open == b.pr_open)
            && (a.had_pr == b.had_pr)
            && (a.worktree == b.worktree)
            && (a.rechecks == b.rechecks)
            && (a.review_restarted == b.review_restarted)
            && (a.credit == b.credit)
            && (a.priority == b.priority)
            && (a.review_stop == b.review_stop)
            && (a.planning_started == b.planning_started)
            && (a.waiting_prerequisite == b.waiting_prerequisite)
            && (a.fx_failures == 0)
            && (a.fx_incidents == 0)
            && (a.effect == if b.adopted { b.effect } else { match b.effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None } })) }),
] hide [settle_shape, current_item, items] {
    kernel_snapshot! { before_confirmed = *m }
    m.fx_failures = 0;
    m.fx_incidents = 0;
    match kind {
        EffectKind::StopRun => {
            m.run = None;
            m.pool = false;
        }
        EffectKind::WithdrawJob => {
            m.job = None;
        }
        EffectKind::ClosePr => {
            m.pr_open = false;
        }
        EffectKind::RemoveWorktree => {
            m.worktree = false;
        }
        _ => {}
    }
    kernel_proof! { { assert(port_acks_specs::cleanup_post(before_confirmed@, *m, kind)) by { reveal(port_acks_specs::cleanup_post); reveal(port_acks_specs::confirmed_cleanup); } }
         }
    kernel_snapshot! { cleaned = *m }
    confirmed_agenda(m, kind);
    kernel_proof! { port_acks_frames::confirmed_result(before_confirmed@, cleaned@, *m, kind);
        port_acks_frames::confirmed_contract(before_confirmed@, cleaned@, *m, kind);
        port_acks_frames::adoption_fields(before_confirmed@, *m, kind, true); }
}
}

kernel_fn! {
/// Confirmed-effect rows / #771: after resource cleanup, consume the same
/// matching agenda item or enter Checking for an adopted worktree.
// The pinned Verus frontend requires nested if-let instead of let chains.
#[allow(clippy::collapsible_if)]
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn confirmed_agenda(m: &mut Node, kind: EffectKind) -> (result: ()) [
    requires old(m).generation < u64::MAX,
        kind == EffectKind::StopRun ==> !old(m).pool,
    ensures port_acks_specs::agenda_post(*old(m), *final(m), kind),
        (matches!(kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
            ==> distance(*final(m)) == distance(*old(m))),
        (old(m).effect.is_none() && old(m).question.is_none() && !old(m).waiting_incident
            && (!old(m).paused || old(m).phase is Stopping)
            && (current_item(*old(m)) == Some(Item::Do(kind))
                || current_item(*old(m)) == Some(Item::Teardown(kind)))
            ==> distance(*final(m)) + 2 <= distance(*old(m))),
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m))
            && (old(m).phase is Stopping ==> current_item(*old(m)) == Some(Item::Teardown(kind)))
            ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).generation >= old(m).generation,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).question == old(m).question,
        final(m).fx_failures == old(m).fx_failures,
        final(m).fx_incidents == old(m).fx_incidents,
        final(m).last_cmd == old(m).last_cmd,
        // #806 adoption: neutral field guarantees consumed by the adoption owner.
        ({ let b = *old(m); let a = *final(m);
            (b.phase == Phase::Making && b.pos == 0 && b.proposed.is_some() && kind == EffectKind::PrepareWorktree) ==> ((a.adopted)
            && (a.head == if b.adopted { b.head } else { b.proposed })
            && (a.phase == if b.adopted { Phase::Making } else { Phase::Checking })
            && (a.pos == if b.adopted { 1u8 } else { 0u8 })
            && (a.generation as int == b.generation as int + if b.adopted { 0int } else { 1int })
            && (a.ticket == if b.adopted { b.ticket } else { None })
            && (a.checked == if b.adopted { b.checked } else { None })
            && (a.repo == b.repo)
            && (a.footprint == b.footprint)
            && (a.next_seq == b.next_seq)
            && (a.run == b.run)
            && (a.job == b.job)
            && (a.pool == b.pool)
            && (a.holds == b.holds)
            && (a.paused == b.paused)
            && (a.last_cmd == b.last_cmd)
            && (a.question == b.question)
            && (a.attempt == b.attempt)
            && (a.round == b.round)
            && (a.replanned == b.replanned)
            && (a.plan_retries == b.plan_retries)
            && (a.audit_rounds == b.audit_rounds)
            && (a.early_end == b.early_end)
            && (a.run_failures == b.run_failures)
            && (a.run_incidents == b.run_incidents)
            && (a.waiting_incident == b.waiting_incident)
            && (a.incident_needs_rob == b.incident_needs_rob)
            && (a.proposed == b.proposed)
            && (a.approved == b.approved)
            && (a.merged == b.merged)
            && (a.pr_open == b.pr_open)
            && (a.had_pr == b.had_pr)
            && (a.worktree == b.worktree)
            && (a.rechecks == b.rechecks)
            && (a.review_restarted == b.review_restarted)
            && (a.credit == b.credit)
            && (a.priority == b.priority)
            && (a.review_stop == b.review_stop)
            && (a.planning_started == b.planning_started)
            && (a.waiting_prerequisite == b.waiting_prerequisite)
            && (a.fx_failures == b.fx_failures)
            && (a.fx_incidents == b.fx_incidents)
            && (a.effect == if b.adopted { b.effect } else { match b.effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None } })) }),
] hide [current_item, items, settle_shape] {
    kernel_snapshot! { before_agenda = *m }
    confirmed_agenda_body(m, kind);
    kernel_proof! {
        port_acks_frames::agenda_contract(before_agenda@, *m, kind);
        port_acks_frames::adoption_fields(before_agenda@, *m, kind, false);
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
    ensures old(w).nodes@[i as int].effect.is_none() ==> port_acks_specs::retry_post(old(w).nodes@[i as int], final(w).nodes@[i as int], kind),
        old(w).nodes@[i as int].effect.is_none() ==> port_acks_specs::retry_output(old(w).nodes@[i as int], i, kind, *old(out), *final(out)),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
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
    kernel_snapshot! { retry_before = w.nodes@[i as int] }
    kernel_snapshot! { retry_out_before = *out }
    match failed_delivery_node(&mut w.nodes[i], kind) {
        None => {
            kernel_snapshot! { retry_mid = w.nodes@[i as int] }
            emit(w, i, kind, out);
            kernel_proof! { if retry_before@.effect.is_none() { port_acks_frames::retry_emitted_result(retry_before@, retry_mid@,
                w.nodes@[i as int], kind, retry_out_before@, *out, i); } }
        },
        Some(needs_rob) => {
            kernel_snapshot! { retry_mid = w.nodes@[i as int] }
            raise_incident(w, i, needs_rob, out);
            kernel_proof! { if retry_before@.effect.is_none() { port_acks_frames::retry_incident_result(retry_before@, retry_mid@,
                w.nodes@[i as int], kind, retry_out_before@, *out, i, needs_rob); } }
        },
    }
}
}

kernel_fn! {
/// Failed-delivery rows / #771: reset the retry ladder before emitting or
/// raising its incident. This node-local part allocates at most one generation.
#[allow(clippy::needless_else)] // The else carries a Verus proof and erases from ordinary Rust.
fn failed_delivery_node(m: &mut Node, kind: EffectKind) -> (result: Option<bool>) [
    requires old(m).generation < u64::MAX,
        old(m).fx_failures <= 1, old(m).fx_incidents <= 2,
    ensures old(m).effect.is_none() ==> port_acks_specs::delivery_post(*old(m), *final(m), kind),
        result == if old(m).fx_failures == 0 { None } else { Some(final(m).fx_incidents >= 2) },
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).question == old(m).question, final(m).last_cmd == old(m).last_cmd,
        final(m).fx_failures <= 1, final(m).fx_incidents <= 2,
        result.is_none() == (old(m).fx_failures == 0),
] {
    kernel_snapshot! { before_delivery = *m }
    m.fx_failures += 1;
    if m.fx_failures < 2 {
        kernel_proof! { if before_delivery@.effect.is_none() { assert(port_acks_specs::delivery_post(before_delivery@, *m, kind)) by { reveal(port_acks_specs::delivery_post); reveal(port_acks_specs::failed_delivery); } } }
        return None;
    }
    m.fx_failures = 0;
    m.fx_incidents = (m.fx_incidents + 1).min(2);
    kernel_snapshot! { counted_delivery = *m }
    kernel_proof! { if before_delivery@.effect.is_none() { assert(port_acks_specs::counted_post(before_delivery@, counted_delivery@)) by { reveal(port_acks_specs::counted_post); } } }
    let needs_rob = m.fx_incidents >= 2;
    if kind.starts_operation() {
        abandon_operation(m);
        kernel_snapshot! { abandoned_delivery = *m }
        m.pos = operation_start(m.phase);
        m.generation += 1;
        kernel_proof! { if before_delivery@.effect.is_none() {
            assert(port_acks_specs::delivery_post(before_delivery@, *m, kind)) by { reveal(port_acks_specs::delivery_post); reveal(port_acks_specs::failed_delivery); reveal(port_acks_specs::counted_post); }
        } }
    } else {
        kernel_proof! { if before_delivery@.effect.is_none() {
            assert(port_acks_specs::delivery_post(before_delivery@, *m, kind)) by { reveal(port_acks_specs::delivery_post); reveal(port_acks_specs::failed_delivery); reveal(port_acks_specs::counted_post); }
        } }
    }
    Some(needs_rob)
}
}

kernel_fn! {
/// Permanent-effect-failure rows / #771: exactly one question, no credit change.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn effect_failed(w: &mut World, i: usize, kind: EffectKind,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        old(w).nodes@[i as int].generation < u64::MAX,
        old(w).next_question < u64::MAX,
    ensures old(w).nodes@[i as int].effect.is_none() ==> port_acks_specs::permanent_post(old(w).nodes@[i as int], final(w).nodes@[i as int], old(w).next_question, kind),
        port_acks_specs::appended(*old(out), *final(out), port_acks_specs::permanent_emissions(old(w).next_question, i)),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
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
] hide [port_acks_specs::failed_cont] {
    kernel_snapshot! { permanent_before = w.nodes@[i as int] }
    kernel_snapshot! { permanent_out_before = *out }
    kernel_snapshot! { permanent_question_before = w.next_question }
    notice(out, i, Notice::Incident { needs_rob: true });
    kernel_snapshot! { incident_out = *out }
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
        kernel_proof! {
            if permanent_before@.effect.is_none() { assert(port_acks_specs::permanent_post(permanent_before@, w.nodes@[i as int], id, kind)) by { reveal(port_acks_specs::permanent_post); reveal(port_acks_specs::permanent_node); } }
            port_acks_frames::two_notices_result(permanent_out_before@, incident_out@, *out, i, id);
        }
        return;
    }
    let m = &mut w.nodes[i];
    if kind.starts_operation() {
        abandon_operation(m);
    }
    kernel_snapshot! { permanent_abandoned = *m }
    kernel_proof! { if permanent_before@.effect.is_none() {
        assert(port_acks_specs::permanent_abandon_post(permanent_before@, permanent_abandoned@, kind)) by {
            reveal(port_acks_specs::permanent_abandon_post);
        }
    } }
    let cont = match m.phase {
        Phase::Ingesting => Cont::Ingesting,
        Phase::Auditing => Cont::Auditing,
        Phase::Planning { must_split } => Cont::Planning { must_split },
        phase => match phase.resume() {
            Some(r) => Cont::Ready(r),
            None => Cont::Unplanned,
        },
    };
    kernel_proof! { assert(cont == port_acks_specs::failed_cont(permanent_before@)) by { reveal(port_acks_specs::failed_cont); } }
    ask_rob(w, i, QKind::EffectFailed, cont, 0, out);
    kernel_proof! {
        if permanent_before@.effect.is_none() {
            assert(port_acks_specs::asked_post(permanent_abandoned@, w.nodes@[i as int], permanent_question_before@, cont)) by {
                reveal(port_acks_specs::asked_post);
            }
            port_acks_frames::permanent_composed(permanent_before@, permanent_abandoned@,
                w.nodes@[i as int], permanent_question_before@, kind, cont);
        }
        port_acks_frames::two_notices_result(permanent_out_before@, incident_out@, *out, i, permanent_question_before@);
    }
}
}

kernel_fn! {
/// State-table live Ack rows / #806: verbatim match extracted under the 11-06 standing rule.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn apply_live_ack(w: &mut World, i: usize, e: InFlight, ack: Ack, out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]), arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX, old(w).nodes@[i as int].effect.is_none(), !e.cancelling,
    ensures port_acks_specs::ack_local(Node { effect: Some(e), ..old(w).nodes@[i as int] },
        final(w).nodes@[i as int], old(w).next_question, final(w).next_question,
        i, e.seq, ack, *old(out), *final(out)),
        ack == Ack::Confirmed ==> *final(out) == *old(out) && final(w).next_question == old(w).next_question,
        (ack == Ack::Confirmed && matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
            ==> distance(final(w).nodes@[i as int]) == distance(old(w).nodes@[i as int])),
        (ack == Ack::Confirmed && old(w).nodes@[i as int].question.is_none() && !old(w).nodes@[i as int].waiting_incident
            && (!old(w).nodes@[i as int].paused || old(w).nodes@[i as int].phase is Stopping)
            && (current_item(old(w).nodes@[i as int]) == Some(Item::Do(e.kind))
                || current_item(old(w).nodes@[i as int]) == Some(Item::Teardown(e.kind)))
            ==> distance(final(w).nodes@[i as int]) + 2 <= distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int])
            && (old(w).nodes@[i as int].phase is Stopping ==> current_item(old(w).nodes@[i as int]) == Some(Item::Teardown(e.kind)))
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
] hide [current_item, items, settle_shape] {
    kernel_snapshot! { live_before = w.nodes@[i as int] }
    kernel_snapshot! { live_original = Node { effect: Some(e), ..w.nodes@[i as int] } }
    kernel_snapshot! { live_question = w.next_question }
    kernel_snapshot! { live_out = *out }
    kernel_proof! { assert(port_acks_specs::cleared_post(live_original@, live_before@)) by { reveal(port_acks_specs::cleared_post); } }
    match ack {
        Ack::Confirmed => {
            confirmed(&mut w.nodes[i], e.kind); kernel_proof! { port_acks_frames::confirmed_live(live_original@, live_before@, w.nodes@[i as int], live_question@, i, e.seq, live_out@, e); }

        },
        Ack::Retryable => { delivery_failed(w, i, e.kind, out); kernel_proof! { port_acks_frames::retry_live(live_original@, live_before@, w.nodes@[i as int], live_question@, i, e.seq, live_out@, *out, e); }  },
        Ack::Permanent => { effect_failed(w, i, e.kind, out); kernel_proof! { port_acks_frames::permanent_live(live_original@, live_before@, w.nodes@[i as int], live_question@, w.next_question, i, e.seq, ack, live_out@, *out, e); }  },
        Ack::BodyPolicyRefused if e.kind == EffectKind::OpenPr => {
            stop_for_review(&mut w.nodes[i], ReviewStop::PublicationBody); kernel_proof! { port_acks_frames::refused_live(live_original@, live_before@, w.nodes@[i as int], live_question@, i, e.seq, live_out@, e); }

        }
        Ack::BodyPolicyRefused => { effect_failed(w, i, e.kind, out); kernel_proof! { port_acks_frames::permanent_live(live_original@, live_before@, w.nodes@[i as int], live_question@, w.next_question, i, e.seq, ack, live_out@, *out, e); }  },
    }
}
}

kernel_fn! {
/// State-table adoption row / #806: four verbatim statements, behaviour-identical standing rule.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn adopt_proposed_worktree(m: &mut Node, head: HeadId) -> (result: ()) [
    requires old(m).generation < u64::MAX, !old(m).adopted, old(m).proposed == Some(head),
        current_item(*old(m)) == Some(Item::Do(EffectKind::PrepareWorktree))
            || current_item(*old(m)) == Some(Item::Teardown(EffectKind::PrepareWorktree)),
    ensures port_acks_specs::agenda_post(*old(m), *final(m), EffectKind::PrepareWorktree),
] {
    kernel_snapshot! { adoption_before = *m }
    m.adopted = true;
    m.head = Some(head);
    m.checked = None;


    kernel_snapshot! { adoption_mid = *m }
    kernel_proof! { assert(port_acks_specs::adoption_post(adoption_before@, adoption_mid@)) by { reveal(port_acks_specs::adoption_post); } }
    set_phase(m, Phase::Checking);
    kernel_proof! { port_acks_frames::agenda_adoption_composed(adoption_before@, adoption_mid@, *m, EffectKind::PrepareWorktree); }
}
}

#[cfg(test)]
#[path = "../tests/acknowledgements_equivalence/cases.rs"]
mod equivalence;

kernel_fn! {
/// #806 confirmed agenda: verbatim state-writing body isolated from the public field proof.
#[allow(clippy::collapsible_if)]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn confirmed_agenda_body(m: &mut Node, kind: EffectKind) -> (result: ()) [
    requires old(m).generation < u64::MAX,
        kind == EffectKind::StopRun ==> !old(m).pool,
    ensures port_acks_specs::agenda_post(*old(m), *final(m), kind),
] hide [current_item, items] {
    kernel_snapshot! { before_agenda = *m }
    kernel_proof! { port_acks_frames::agenda_input(*m); }
    if !kind.starts_operation() {
        if let Some(Item::Do(k) | Item::Teardown(k)) = m.current() {
            if k == kind {
                if kind == EffectKind::PrepareWorktree && !m.adopted {
                    if let Some(head) = m.proposed {
                        adopt_proposed_worktree(m, head);

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

    }
}
}

kernel_fn! {
/// #806 Ack rows: verbatim state-writing body isolated from the public adoption field proof.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn on_ack_body(w: &mut World, i: usize, seq: u64, ack: Ack,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures port_acks_specs::ack_local(old(w).nodes@[i as int], final(w).nodes@[i as int],
        old(w).next_question, final(w).next_question, i, seq, ack, *old(out), *final(out)),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 2,
        ack == Ack::Confirmed ==> *final(out) == *old(out) && final(w).next_question == old(w).next_question,
] hide [current_item, items, settle_shape, node_headroom, arithmetic_consistent] {
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

            port_acks_frames::cancelled_ack(ack_before@.nodes@[i as int], w.nodes@[i as int],
                w.next_question, i, seq, ack, ack_out_before@, e);
        }
        return;
    }
    apply_live_ack(w, i, e, ack, out);
    kernel_proof! {
        port_acks_frames::lift_live_local(ack_before@.nodes@[i as int], ack_cleared@,
            w.nodes@[i as int], ack_before@.next_question, w.next_question, i, seq, ack, ack_out_before@, *out, e);
    }

}
}
