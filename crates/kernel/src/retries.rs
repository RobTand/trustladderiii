//! Executable retries transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
/// Failed-run rows / #771: a retry and incident allocate at most two generations.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn run_failed(w: &mut World, i: usize, out: &mut Outbox, report: &mut Report) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        old(w).nodes@[i as int].generation as int + 2 <= u64::MAX as int,
        old(w).nodes@[i as int].run_failures <= 1,
        old(w).nodes@[i as int].run_incidents <= 2,
    ensures report::failed_round(old(report).entries@, final(report).entries@,
            NodeId(i as u64), FailedRoundCause::RunFailed),
        port_t860_contracts::failed_post(old(w).nodes@[i as int], final(w).nodes@[i as int], *old(out), *final(out)),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).nodes@[i as int].question == old(w).nodes@[i as int].question,
        final(w).nodes@[i as int].run_failures <= 1,
        final(w).nodes@[i as int].run_incidents <= 2,
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
    report.record_failed_round(NodeId(i as u64), FailedRoundCause::RunFailed);
    kernel_snapshot! { before_failed = w.nodes@[i as int] }
    kernel_snapshot! { before_failed_out = *out }
    let m = &mut w.nodes[i];
    abandon_operation(m);
    m.pos = operation_start(m.phase);
    m.generation += 1;
    m.run_failures += 1;
    if m.run_failures >= 2 {
        // Every second failure raises an incident. Only Rob can clear the second one.
        m.run_failures = 0;
        m.run_incidents = (m.run_incidents + 1).min(2);
        let needs_rob = m.run_incidents >= 2;
        kernel_proof! {
            assert(retirement::incident(*m, needs_rob) == retries_facts::failed_run(before_failed@)) by {
                retries_facts::incident(before_failed@, *m, needs_rob);
            }
        }
        kernel_snapshot! { ready = w.nodes@ }
        raise_failed_run_incident(w, i, needs_rob, out);
        kernel_proof! {
            retries_facts::finish_failed_incident(before_failed@, ready@, w.nodes@,
                i as int, needs_rob, before_failed_out@, *out);
        }
    }
    kernel_proof! {
        if before_failed@.run_failures == 0 {
            assert(w.nodes@[i as int] == retries_facts::failed_run(before_failed@)) by {
                retries_facts::restarted(before_failed@, w.nodes@[i as int]);
            }
            assert(port_t860_contracts::failed_post(before_failed@, w.nodes@[i as int], before_failed_out@, *out)) by {
                retries_facts::failed_result(before_failed@, w.nodes@[i as int], before_failed_out@, *out);
            }
        }
    }
}
}

kernel_fn! {
/// Private query boundary for the failed-run incident composition.
fn raise_failed_run_incident(w: &mut World, i: usize, needs_rob: bool,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        holding(old(w).nodes@[i as int].phase) ==>
            old(w).nodes@[i as int].generation < u64::MAX,
    ensures retries_facts::incident_write(old(w).nodes@, final(w).nodes@, i as int, needs_rob),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@[i as int].generation as int
            == old(w).nodes@[i as int].generation as int
                + if holding(old(w).nodes@[i as int].phase) { 1int } else { 0int },
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).nodes@[i as int].question == old(w).nodes@[i as int].question,
        final(w).nodes@[i as int].waiting_incident,
        final(w).nodes@[i as int].incident_needs_rob == needs_rob,
        final(w).nodes@[i as int].ticket.is_none(),
        final(w).nodes@[i as int].run_failures == old(w).nodes@[i as int].run_failures,
        final(w).nodes@[i as int].run_incidents == old(w).nodes@[i as int].run_incidents,
        final(w).nodes@[i as int].fx_failures == old(w).nodes@[i as int].fx_failures,
        final(w).nodes@[i as int].fx_incidents == old(w).nodes@[i as int].fx_incidents,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len == old(out).len + 1,
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] {
    kernel_snapshot! { before_nodes = w.nodes@ }
    raise_incident(w, i, needs_rob, out);
    kernel_proof! {
        retries_facts::pack_incident_write(before_nodes@, w.nodes@, i as int, needs_rob);
    }
}
}

kernel_fn! {
/// Q54 / #771: stopping for review spends no credit and allocates at most one generation.
/// Under `fail_fast`, a step that would retry the work stops for TL3 review.
/// It asks Rob nothing; the summary offers `restart`, as for any stop (Q49).
pub(super) fn stop_for_review(m: &mut Node, step: ReviewStop) -> (result: ()) [
    requires old(m).generation < u64::MAX,
    ensures *final(m) == retirement::stopped(Node { review_stop: Some(step), ..*old(m) }, Outcome::Stopped),
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).last_cmd == old(m).last_cmd, final(m).review_stop == Some(step),
        !(old(m).phase is Closed || old(m).phase is Stopping) ==>
            final(m).phase == Phase::Stopping(Outcome::Stopped) && final(m).question.is_none(),
        final(m).plan_retries == old(m).plan_retries,
] proof [reveal(retirement::stopped);] {
    m.review_stop = Some(step);
    enter_stopping(m, Outcome::Stopped);
}
}

kernel_fn! {
/// Failed-round rows / #771: the free fix and paid ladder share one-step bounds.
pub(super) fn failed_round(w: &mut World, i: usize, step: ReviewStop,
    out: &mut Outbox, report: &mut Report) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures report::failed_round(old(report).entries@, final(report).entries@,
            NodeId(i as u64), report::failure_cause(step)),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        !(old(w).nodes@[i as int].phase is Closed || old(w).nodes@[i as int].phase is Stopping) ==> handler_safety::ladder(old(w).fail_fast, old(w).nodes@[i as int].round != 0, old(w).nodes@[i as int], final(w).nodes@[i as int], *old(out), *final(out)),
] proof [reveal(retry_bounds);] {
    kernel_snapshot! { before_failed = w.nodes@[i as int] }
    kernel_snapshot! { before_failed_out = *out }
    kernel_snapshot! { before_report = report.entries@ }
    kernel_proof! { report::lemmas::failure_kind(step); }
    let cause = match step {
        ReviewStop::ChecksFailed => FailedRoundCause::ChecksFailed,
        ReviewStop::ChangesRequested => FailedRoundCause::ChangesRequested,
        ReviewStop::CiFailed => FailedRoundCause::CiFailed,
        _ => FailedRoundCause::MakerCantMeet,
    };
    if step != ReviewStop::MakerCantMeet || w.nodes[i].round == 0 {
        report.record_failed_round(NodeId(i as u64), cause);
        kernel_proof! {
            report::lemmas::read_failed_round(before_report@, report.entries@,
                NodeId(i as u64), cause);
        }
    }
    let m = &mut w.nodes[i];
    if m.round == 0 {
        // The fix round resumes the maker's session. It's free.
        m.round = 1;
        set_phase(m, if m.holds { Phase::Making } else { Phase::Ready(Resume::Making) });
        kernel_proof! {
            handler_safety::same_ladder(w.fail_fast, before_failed@, w.nodes@[i as int], before_failed_out@, *out);
        }
    } else {
        kernel_snapshot! { before_rung_report = report.entries@ }
        next_rung(w, i, step, out, report);
        kernel_proof! {
            if !(before_failed@.phase is Closed || before_failed@.phase is Stopping) {
                handler_safety::rung_ladder(w.fail_fast, before_failed@, w.nodes@[i as int], before_failed_out@, *out);
            }
        }
        kernel_proof! {
            report::lemmas::round_delta(before_rung_report@, report.entries@,
                NodeId(i as u64),
                if step == ReviewStop::MakerCantMeet { Some(FailedRoundCause::MakerCantMeet) }
                else { None });
        }
    }
    kernel_proof! {
        report::lemmas::append_failed_round(before_report@, report.entries@,
            NodeId(i as u64), cause);
    }
}
}

kernel_fn! {
/// Q54 / #771: the ladder after a failed fix round costs at most one credit.
/// It tries a bigger budget, another model, one re-plan, then asks Rob.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn next_rung(w: &mut World, i: usize, step: ReviewStop,
    out: &mut Outbox, report: &mut Report) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures report::forwards(old(report).entries@, final(report).entries@,
            report::round_entries(NodeId(i as u64),
                if step == ReviewStop::MakerCantMeet { Some(FailedRoundCause::MakerCantMeet) }
                else { None })),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        !(old(w).nodes@[i as int].phase is Closed || old(w).nodes@[i as int].phase is Stopping) && old(w).fail_fast ==> final(w).nodes@[i as int].review_stop.is_some() && final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit
            && final(out).len == old(out).len && final(w).nodes@[i as int].phase == Phase::Stopping(Outcome::Stopped)
            && final(w).nodes@[i as int].question.is_none(),
        !(old(w).nodes@[i as int].phase is Closed || old(w).nodes@[i as int].phase is Stopping) && !old(w).fail_fast ==> transition_safety::retried(old(w).nodes@[i as int], final(w).nodes@[i as int]) && final(w).nodes@[i as int].review_stop == old(w).nodes@[i as int].review_stop,
] proof [reveal(retry_bounds);] {
    kernel_snapshot! { before_report = report.entries@ }
    if step == ReviewStop::MakerCantMeet {
        report.record_failed_round(NodeId(i as u64), FailedRoundCause::MakerCantMeet);
        kernel_proof! { report::lemmas::read_failed_round(before_report@, report.entries@,
            NodeId(i as u64), FailedRoundCause::MakerCantMeet); }
    }
    kernel_proof! {
        report::lemmas::forward_round(before_report@, report.entries@, NodeId(i as u64),
            if step == ReviewStop::MakerCantMeet { Some(FailedRoundCause::MakerCantMeet) }
            else { None });
    }
    let fail_fast = w.fail_fast;
    let m = &mut w.nodes[i];
    if fail_fast {
        stop_for_review(m, step);
    } else if m.attempt < 3 {
        m.attempt += 1;
        m.review_restarted = false;
        m.round = 0;
        if m.credit.can_spend(1) {
            m.credit.spend(1);
            set_phase(m, if m.holds { Phase::Making } else { Phase::Ready(Resume::Making) });
        } else {
            ask_rob(
                w,
                i,
                QKind::OutOfCredit,
                Cont::Ready(Resume::Making),
                1,
                out,
            );
        }
    } else if !m.replanned {
        m.replanned = true;
        m.holds = false;
        if m.credit.can_spend(1) {
            m.credit.spend(1);
            m.plan_retries = 0;
            set_phase(m, Phase::Planning { must_split: false });
            notice(out, i, Notice::Note(NoteKind::Replanned));
        } else {
            ask_rob(
                w,
                i,
                QKind::OutOfCredit,
                Cont::Planning { must_split: false },
                1,
                out,
            );
        }
    } else {
        ask_rob(w, i, QKind::CantMeet, Cont::Ready(Resume::Making), 1, out);
    }
}
}

kernel_fn! {
/// Q54 / #771: exhaustion either stops, pays once to re-plan, or asks Rob.
pub(super) fn out_of_room(w: &mut World, i: usize, step: ReviewStop,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        !(old(w).nodes@[i as int].phase is Closed || old(w).nodes@[i as int].phase is Stopping) ==> handler_safety::ladder(old(w).fail_fast, true, old(w).nodes@[i as int], final(w).nodes@[i as int], *old(out), *final(out)),
        !old(w).fail_fast && !transition_safety::can_spend_one(old(w).nodes@[i as int]) ==> final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit
            && final(w).nodes@[i as int].review_stop == old(w).nodes@[i as int].review_stop
            && match final(w).nodes@[i as int].question {
                Some(q) => q.kind == QKind::OutOfCredit && q.cont == Cont::Planning { must_split: true },
                None => false },
] proof [reveal(retry_bounds);] {
    kernel_snapshot! { before_room = w.nodes@[i as int] }
    kernel_snapshot! { before_room_out = *out }
    let fail_fast = w.fail_fast;
    let m = &mut w.nodes[i];
    if fail_fast {
        stop_for_review(m, step);
        kernel_proof! {
            if !(before_room@.phase is Closed || before_room@.phase is Stopping) {
                handler_safety::stop_ladder(before_room@, step, before_room_out@);
            }
        }
        return;
    }
    m.holds = false;
    if m.credit.can_spend(1) {
        m.credit.spend(1);
        m.plan_retries = 0;
        set_phase(m, Phase::Planning { must_split: true });
    } else {
        ask_rob(
            w,
            i,
            QKind::OutOfCredit,
            Cont::Planning { must_split: true },
            1,
            out,
        );
    }
    kernel_proof! {
        handler_safety::retry_ladder(before_room@, w.nodes@[i as int], before_room_out@, *out);
    }
}
}

kernel_fn! {
/// Q54 / #771: planner retries saturate their local retry count.
/// Under `fail_fast`, `cant_meet` stops for review. A plan that does not fit
/// keeps the existing retry rule; the live worker reports broken rules as `cant_meet`.
pub(super) fn plan_retry(w: &mut World, i: usize, cant_meet: bool,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        old(w).nodes@[i as int].phase is Planning ==> handler_safety::ladder(old(w).fail_fast, cant_meet, old(w).nodes@[i as int], final(w).nodes@[i as int], *old(out), *final(out)),
        old(w).fail_fast && cant_meet && old(w).nodes@[i as int].phase is Planning ==>
            final(w).nodes@[i as int].phase == Phase::Stopping(Outcome::Stopped)
                && final(w).nodes@[i as int].review_stop == Some(ReviewStop::PlanCantMeet),
] proof [reveal(retry_bounds);] {
    kernel_snapshot! { before_plan = w.nodes@[i as int] }
    kernel_snapshot! { before_plan_out = *out }
    let fail_fast = w.fail_fast;
    let m = &mut w.nodes[i];
    m.plan_retries = m.plan_retries.saturating_add(1);
    let phase = m.phase;
    kernel_snapshot! { counted_plan = *m }
    if fail_fast && cant_meet {
        stop_for_review(m, ReviewStop::PlanCantMeet);
        kernel_proof! {
            if before_plan@.phase is Planning {
                handler_safety::stop_ladder(counted_plan@, ReviewStop::PlanCantMeet, before_plan_out@);
                handler_safety::ladder_rebase(fail_fast, cant_meet, before_plan@, counted_plan@,
                    w.nodes@[i as int], before_plan_out@, *out);
            }
        }
    } else if m.plan_retries < 2 {
        // Re-run once, with the reason or the next model on the planner's ladder.
        set_phase(m, phase);
    } else {
        m.plan_retries = 0;
        let must_split = matches!(phase, Phase::Planning { must_split: true });
        ask_rob(w, i, QKind::CantMeet, Cont::Planning { must_split }, 1, out);
    }
    kernel_proof! {
        if before_plan@.phase is Planning && !(fail_fast && cant_meet) {
            if cant_meet {
                handler_safety::retry_ladder(before_plan@, w.nodes@[i as int], before_plan_out@, *out);
            } else {
                handler_safety::same_ladder(fail_fast, before_plan@, w.nodes@[i as int], before_plan_out@, *out);
            }
        }
    }
}
}

kernel_fn! {
/// Recheck rows / #771: a new head or reviewer uses the same free allowance.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn recheck(w: &mut World, i: usize, target: Phase,
    out: &mut Outbox) -> (result: ()) [
    requires !(target is Closed), i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures (ordinary_await(old(w).nodes@[i as int]) && old(w).nodes@[i as int].phase == Phase::Rebasing
            && target == Phase::Checking
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        handler_safety::keeps_review_stop(old(w).nodes@[i as int], final(w).nodes@[i as int]),
] proof [reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(retry_bounds);] {
    kernel_snapshot! { before_recheck = *w }
    let m = &mut w.nodes[i];
    m.rechecks = m.rechecks.saturating_add(1);
    let free = m.rechecks <= FREE_RECHECKS;
    if free || m.credit.can_spend(1) {
        if !free {
            m.credit.spend(1);
        }
        if !target.holding() {
            m.holds = false;
        }
        set_phase(m, target);
        kernel_proof! {
            handler_safety::keep_review_stop(before_recheck@.nodes@[i as int], w.nodes@[i as int]);
        }
    } else {
        ask_rob(
            w,
            i,
            QKind::OutOfCredit,
            Cont::Ready(if target == Phase::Reviewing {
                Resume::Reviewing
            } else {
                Resume::Checking
            }),
            1,
            out,
        );
        kernel_proof! {
            handler_safety::keep_review_stop(before_recheck@.nodes@[i as int], w.nodes@[i as int]);
        }
    }
    kernel_proof! {
        assert(w.nodes@.len() == before_recheck@.nodes@.len());
        assert forall|j: int| 0 <= j < before_recheck@.nodes@.len() && j != i implies
            w.nodes@[j] == before_recheck@.nodes@[j] by {}
        assert(w.nodes@[i as int].generation as int
            <= before_recheck@.nodes@[i as int].generation as int + 1);
        assert(w.nodes@[i as int].credit.spent as int
            <= before_recheck@.nodes@[i as int].credit.spent as int + 1);
        assert(w.next_question as int <= before_recheck@.next_question as int + 1);
    }

}
}
