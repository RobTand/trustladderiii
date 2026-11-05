//! Executable retries transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
/// Failed-run rows / #771: a retry and incident allocate at most two generations.
pub(super) fn run_failed(w: &mut World, i: usize, out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        old(w).nodes@[i as int].generation as int + 2 <= u64::MAX as int,
        old(w).nodes@[i as int].run_failures <= 1,
        old(w).nodes@[i as int].run_incidents <= 2,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
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
        raise_incident(w, i, needs_rob, out);
    }
}
}

kernel_fn! {
/// Q54 / #771: stopping for review spends no credit and allocates at most one generation.
/// Under `fail_fast`, a step that would retry the work stops for TL3 review.
/// It asks Rob nothing; the summary offers `restart`, as for any stop (Q49).
pub(super) fn stop_for_review(m: &mut Node, step: ReviewStop) -> (result: ()) [
    requires old(m).generation < u64::MAX,
    ensures final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).last_cmd == old(m).last_cmd, final(m).review_stop == Some(step),
        *final(m) == retirement::stopped(Node { review_stop: Some(step), ..*old(m) }, Outcome::Stopped),
] {
    m.review_stop = Some(step);
    enter_stopping(m, Outcome::Stopped);
}
}

kernel_fn! {
/// Failed-round rows / #771: the free fix and paid ladder share one-step bounds.
pub(super) fn failed_round(w: &mut World, i: usize, step: ReviewStop,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(w).nodes@[i as int] == retirement_retry::failed(old(w).nodes@[i as int], old(w).fail_fast, old(w).next_question, step),
        final(w).next_question as int == old(w).next_question as int + if old(w).nodes@[i as int].round != 0 && retirement_retry::rung_asks(old(w).nodes@[i as int], old(w).fail_fast) { 1int } else { 0int },
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
] proof [reveal(retry_bounds); reveal(retirement_retry::failed); ] {
    let m = &mut w.nodes[i];
    if m.round == 0 {
        // The fix round resumes the maker's session. It's free.
        m.round = 1;
        set_phase(m, Phase::Making);
    } else {
        next_rung(w, i, step, out);
    }
}
}

kernel_fn! {
/// Q54 / #771: the ladder after a failed fix round costs at most one credit.
/// It tries a bigger budget, another model, one re-plan, then asks Rob.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn next_rung(w: &mut World, i: usize, step: ReviewStop,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(w).nodes@[i as int] == retirement_retry::rung(old(w).nodes@[i as int], old(w).fail_fast, old(w).next_question, step),
        final(w).next_question as int == old(w).next_question as int + if retirement_retry::rung_asks(old(w).nodes@[i as int], old(w).fail_fast) { 1int } else { 0int },
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
] proof [reveal(retry_bounds); reveal(retirement_retry::rung); ] {
    let fail_fast = w.fail_fast;
    let m = &mut w.nodes[i];
    if fail_fast {
        stop_for_review(m, step);
    } else if m.attempt < 3 {
        next_attempt(w, i, step, out);
    } else if !m.replanned {
        replan_after_fix(w, i, step, out);
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
] proof [reveal(retry_bounds);] {
    let fail_fast = w.fail_fast;
    let m = &mut w.nodes[i];
    if fail_fast {
        stop_for_review(m, step);
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
] proof [reveal(retry_bounds);] {
    let fail_fast = w.fail_fast;
    let m = &mut w.nodes[i];
    m.plan_retries = m.plan_retries.saturating_add(1);
    let phase = m.phase;
    if fail_fast && cant_meet {
        stop_for_review(m, ReviewStop::PlanCantMeet);
    } else if m.plan_retries < 2 {
        // Re-run once, with the reason or the next model on the planner's ladder.
        set_phase(m, phase);
    } else {
        m.plan_retries = 0;
        let must_split = matches!(phase, Phase::Planning { must_split: true });
        ask_rob(w, i, QKind::CantMeet, Cont::Planning { must_split }, 1, out);
    }
}
}

kernel_fn! {
/// Recheck rows / #771: a new head or reviewer uses the same free allowance.
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
        (target == Phase::Ready(Resume::Checking) && retirement_retry::published_source(old(w).nodes@[i as int]))
            ==> final(w).nodes@[i as int]
            == retirement_retry::rechecked(old(w).nodes@[i as int], target, old(w).next_question),
        final(w).next_question as int == old(w).next_question as int
            + if old(w).nodes@[i as int].rechecks >= FREE_RECHECKS && !retirement_retry::can_pay(old(w).nodes@[i as int]) { 1int } else { 0int },
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
 ] proof [reveal(retry_bounds);] {
    kernel_snapshot! { before_recheck = *w }
    let free = increment_rechecks(&mut w.nodes[i]);
    kernel_proof! {
        reveal(distance);
        assert(distance(w.nodes@[i as int]) == distance(before_recheck@.nodes@[i as int]));
    }
    recheck_transition(w, i, target, free, out);
    kernel_proof! {
        reveal(retirement_retry::rechecked);
    }
}
}

kernel_fn! {
/// Q54 / #806: unchanged ladder branch, separately verified over the real writes.
pub(super) fn next_attempt(w: &mut World, i: usize, _step: ReviewStop, out: &mut Outbox) -> (result: ()) [
    requires old(w).nodes@[i as int].attempt < 3,
        i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(w).nodes@[i as int] == retirement_retry::rung(old(w).nodes@[i as int], false, old(w).next_question, _step),
        final(w).next_question as int == old(w).next_question as int + if retirement_retry::rung_asks(old(w).nodes@[i as int], false) { 1int } else { 0int },
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
] proof [reveal(retry_bounds);] {
    kernel_snapshot! { before = *w }
    let m = &mut w.nodes[i];
    m.attempt += 1;
    m.review_restarted = false;
    m.round = 0;
    if m.credit.can_spend(1) {
        m.credit.spend(1);
        set_phase(m, Phase::Making);
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

    kernel_proof! {
        assert(w.nodes@[i as int] == retirement_retry::rung(before@.nodes@[i as int], false, before@.next_question, _step)) by {
            reveal(retirement_retry::rung);
        }
    }

}
}

kernel_fn! {
/// Q54 / #806: unchanged ladder branch, separately verified over the real writes.
pub(super) fn replan_after_fix(w: &mut World, i: usize, _step: ReviewStop, out: &mut Outbox) -> (result: ()) [
    requires old(w).nodes@[i as int].attempt >= 3 && !old(w).nodes@[i as int].replanned,
        i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(w).nodes@[i as int] == retirement_retry::rung(old(w).nodes@[i as int], false, old(w).next_question, _step),
        final(w).next_question as int == old(w).next_question as int + if retirement_retry::rung_asks(old(w).nodes@[i as int], false) { 1int } else { 0int },
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
] proof [reveal(retry_bounds);] {
    kernel_snapshot! { before = *w }
    let m = &mut w.nodes[i];
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

    kernel_proof! {
        assert(w.nodes@[i as int] == retirement_retry::rung(before@.nodes@[i as int], false, before@.next_question, _step)) by {
            reveal(retirement_retry::rung);
        }
    }

}
}

kernel_fn! {
/// Q50 / #806: original saturated counter and free-allowance comparison.
pub(super) fn increment_rechecks(m: &mut Node) -> (result: bool) [
    ensures *final(m) == (Node { rechecks: if old(m).rechecks < 255 { (old(m).rechecks + 1) as u8 } else { 255u8 }, ..*old(m) }),
        result == (final(m).rechecks <= FREE_RECHECKS),
        result == (old(m).rechecks < FREE_RECHECKS),
] {
    m.rechecks = m.rechecks.saturating_add(1);
    m.rechecks <= FREE_RECHECKS
}
}

kernel_fn! {
/// Q50 / #806: original recheck branch after its counter increment, preserving call order.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(60))]
pub(super) fn recheck_transition(w: &mut World, i: usize, target: Phase, free: bool,
    out: &mut Outbox) -> (result: ()) [
    requires free == (old(w).nodes@[i as int].rechecks <= FREE_RECHECKS), !(target is Closed), i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
    ensures (ordinary_await(old(w).nodes@[i as int]) && old(w).nodes@[i as int].phase == Phase::Rebasing
            && target == Phase::Checking
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        retry_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        matches!(target, Phase::Checking | Phase::Ready(Resume::Checking)) ==> final(w).nodes@[i as int] == retirement_retry::recheck_core(old(w).nodes@[i as int], target, free, old(w).next_question),
        final(w).next_question as int == old(w).next_question as int
            + if !free && !retirement_retry::can_pay(old(w).nodes@[i as int]) { 1int } else { 0int },
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
] proof [reveal(retry_bounds); reveal(retirement_retry::continued);] {
    kernel_snapshot! { before = *w }
    let m = &mut w.nodes[i];
    if free || m.credit.can_spend(1) {
        continue_recheck(m, target, free);
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
    }

    kernel_proof! {
        if matches!(target, Phase::Checking | Phase::Ready(Resume::Checking)) {
        assert(w.nodes@[i as int] == retirement_retry::recheck_core(before@.nodes@[i as int], target, free, before@.next_question)) by {
            reveal(retirement_retry::recheck_core);
        }
        }
    }
    kernel_proof! { if ordinary_await(before@.nodes@[i as int]) && before@.nodes@[i as int].phase == Phase::Rebasing && target == Phase::Checking {
        retirement_retry::recheck_distance(before@.nodes@[i as int], target, free, before@.next_question);
    } }

}
}

kernel_fn! {
/// Q50 / #806: publish the exact continuation result from its complete field frame.
pub(super) fn continue_recheck(m: &mut Node, target: Phase, free: bool) -> (result: ()) [
    requires !(target is Closed), old(m).generation < u64::MAX, old(m).credit.spent < u64::MAX,
    ensures *final(m) == retirement_retry::continued(*old(m), target, free),
] {
    kernel_snapshot! { before = *m }
    continue_recheck_fields(m, target, free);
    kernel_proof! { retirement_retry_frame::continuation(before@, *m, target, free); }
}
}

kernel_fn! {
/// Q50 / #806: the unchanged optional debit followed by hold release and phase change.
/// Keep all fields explicit here; the surrounding wrapper proves the Node equality.
pub(super) fn continue_recheck_fields(m: &mut Node, target: Phase, free: bool) -> (result: ()) [
    requires !(target is Closed), old(m).generation < u64::MAX, old(m).credit.spent < u64::MAX,
    ensures final(m).phase == (target),
        final(m).pos == (0),
        final(m).repo == (old(m).repo),
        final(m).footprint == (old(m).footprint),
        final(m).generation == ((old(m).generation + 1) as u64),
        final(m).next_seq == (old(m).next_seq),
        final(m).effect == (match old(m).effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None }),
        final(m).run == (old(m).run),
        final(m).job == (old(m).job),
        final(m).pool == (old(m).pool),
        final(m).ticket == (None),
        final(m).holds == (if holding(target) { old(m).holds } else { false }),
        final(m).paused == (old(m).paused),
        final(m).last_cmd == (old(m).last_cmd),
        final(m).question == (old(m).question),
        final(m).attempt == (old(m).attempt),
        final(m).round == (old(m).round),
        final(m).replanned == (old(m).replanned),
        final(m).plan_retries == (old(m).plan_retries),
        final(m).audit_rounds == (old(m).audit_rounds),
        final(m).early_end == (old(m).early_end),
        final(m).run_failures == (old(m).run_failures),
        final(m).run_incidents == (old(m).run_incidents),
        final(m).fx_failures == (old(m).fx_failures),
        final(m).fx_incidents == (old(m).fx_incidents),
        final(m).waiting_incident == (old(m).waiting_incident),
        final(m).incident_needs_rob == (old(m).incident_needs_rob),
        final(m).proposed == (old(m).proposed),
        final(m).adopted == (old(m).adopted),
        final(m).head == (old(m).head),
        final(m).checked == (old(m).checked),
        final(m).approved == (old(m).approved),
        final(m).merged == (old(m).merged),
        final(m).pr_open == (old(m).pr_open),
        final(m).had_pr == (old(m).had_pr),
        final(m).worktree == (old(m).worktree),
        final(m).rechecks == (old(m).rechecks),
        final(m).review_restarted == (old(m).review_restarted),
        final(m).credit == (if free { old(m).credit } else { Credit { spent: (old(m).credit.spent + 1) as u64, ..old(m).credit } }),
        final(m).priority == (old(m).priority),
        final(m).review_stop == (old(m).review_stop),
        final(m).planning_started == (old(m).planning_started),
        final(m).waiting_prerequisite == (old(m).waiting_prerequisite),
] {
    if !free { m.credit.spend(1); }
    finish_recheck(m, target);
}
}

kernel_fn! {
/// Q50 / #806: the existing hold release immediately followed by phase change.
/// Separate from the optional debit to keep independent branch queries small.
pub(super) fn finish_recheck(m: &mut Node, target: Phase) -> (result: ()) [
    requires old(m).generation < u64::MAX,
    ensures *final(m) == (Node {
        holds: if holding(target) { old(m).holds } else { false },
        generation: (old(m).generation + 1) as u64, phase: target, pos: 0, ticket: None,
        effect: match old(m).effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None },
        ..*old(m)
    }),
] {
    if !target.holding() {
        return finish_recheck_released(m, target);
    }
    finish_recheck_held(m, target)
}
}

kernel_fn! {
/// Q50 / #806: the existing hold release immediately followed by phase change.
/// Separate from the optional debit to keep independent branch queries small.
fn finish_recheck_released(m: &mut Node, target: Phase) -> (result: ()) [
    requires old(m).generation < u64::MAX, !holding(target),
    ensures *final(m) == (Node {
        holds: false,
        generation: (old(m).generation + 1) as u64, phase: target, pos: 0, ticket: None,
        effect: match old(m).effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None },
        ..*old(m)
    }),
] {
    m.holds = false;
    set_phase(m, target);
}
}

kernel_fn! {
/// Q50 / #806: the existing hold release immediately followed by phase change.
/// Separate from the optional debit to keep independent branch queries small.
fn finish_recheck_held(m: &mut Node, target: Phase) -> (result: ()) [
    requires old(m).generation < u64::MAX, holding(target),
    ensures *final(m) == (Node {
        holds: old(m).holds,
        generation: (old(m).generation + 1) as u64, phase: target, pos: 0, ticket: None,
        effect: match old(m).effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None },
        ..*old(m)
    }),
] {
    set_phase(m, target);
}
}
