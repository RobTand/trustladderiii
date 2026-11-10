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
    kernel_snapshot! { before = *w }
    kernel_snapshot! { before_out = *out }
    let change = run_failed_node(w.nodes[i], i, report);
    resolve_reply_change(w, i, change, out);
    kernel_proof! { reply_family_posts::failed_run_props(before@.nodes@[i as int]); }
    kernel_proof! {
        retries_facts::failed_result(before@.nodes@[i as int], w.nodes@[i as int], before_out@, *out);
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
        final(w).nodes@[i as int] == retirement::incident(old(w).nodes@[i as int], needs_rob),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> #[trigger] final(w).nodes@[j] == old(w).nodes@[j],
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
/// Write one returned node and resolve the original world-touching callee.
pub(super) fn resolve_reply_change(w: &mut World, i: usize, change: ReplyChange,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        (change is Ask ==> old(w).next_question < u64::MAX),
        reply_family_values::callable(change),
    ensures final(w).nodes@[i as int] == reply_family_values::finish(old(w).next_question, change),
        final(w).next_question == reply_family_values::next_question(old(w).next_question, change),
        reply_family_values::resolved(change, i as int, *old(out), *final(out)),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).next_ticket == old(w).next_ticket,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
] {
    match change {
        ReplyChange::Done { node, note } => {
            w.nodes[i] = node;
            match note { Some(value) => notice(out, i, value), None => {} }
        },
        ReplyChange::Ask { node, kind, cont, cost } => {
            w.nodes[i] = node;
            ask_rob(w, i, kind, cont, cost, out);
        },
        ReplyChange::Incident { node, needs_rob } => {
            w.nodes[i] = node;
            raise_failed_run_incident(w, i, needs_rob, out);
        },
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
    kernel_snapshot! { before = *w }
    kernel_snapshot! { before_out = *out }
    let change = failed_round_node(w.nodes[i], i, w.fail_fast, w.next_question, step, report);
    resolve_reply_change(w, i, change, out);
    kernel_proof! {
        reveal(retirement::stopped); reveal(retirement::incident);
        reveal(handler_safety::ladder); reveal(handler_safety::keeps_review_stop);
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
    kernel_snapshot! { before = *w }
    kernel_snapshot! { before_out = *out }
    let change = next_rung_node(w.nodes[i], i, w.fail_fast, w.next_question, step, report);
    resolve_reply_change(w, i, change, out);
    kernel_proof! {
        reveal(retirement::stopped); reveal(retirement::incident);
        reveal(handler_safety::ladder); reveal(handler_safety::keeps_review_stop);
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
] {
    kernel_snapshot! { before = *w }
    kernel_snapshot! { before_out = *out }
    let change = out_of_room_node(w.nodes[i], w.fail_fast, w.next_question, step);
    resolve_reply_change(w, i, change, out);
    kernel_proof! {
        reply_family_posts::room_posts(before@.nodes@[i as int], before@.fail_fast,
            before@.next_question, step, i as int, before_out@, *out);
        reply_family_posts::retry_lift(before@, *w, i as int, change, before_out@, *out);
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
    kernel_snapshot! { before = *w }
    kernel_snapshot! { before_out = *out }
    let change = plan_retry_node(w.nodes[i], w.fail_fast, w.next_question, cant_meet);
    resolve_reply_change(w, i, change, out);
    kernel_proof! {
        if before@.nodes@[i as int].phase is Planning {
            reply_family_safety::plan_retry_ladder(before@.nodes@[i as int], before@.fail_fast,
                before@.next_question, cant_meet, change, before_out@, *out, i as int);
        }
    }
    kernel_proof! {
        reveal(retirement::stopped); reveal(retirement::incident);
        reveal(handler_safety::ladder); reveal(handler_safety::keeps_review_stop);
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
    kernel_snapshot! { before = *w }
    kernel_snapshot! { before_out = *out }
    let change = recheck_node(w.nodes[i], w.next_question, target);
    resolve_reply_change(w, i, change, out);
    kernel_proof! {
        reveal(retirement::stopped); reveal(retirement::incident);
        reveal(handler_safety::ladder); reveal(handler_safety::keeps_review_stop);
    }
}
}

kernel_fn! {
/// Canonical event-independent run failed node work.
pub(super) fn run_failed_node(n: Node, i: usize, report: &mut Report) -> (result: ReplyChange) [
    requires n.generation as int + 2 <= u64::MAX as int, n.run_failures <= 1, n.run_incidents <= 2,
    ensures report::failed_round(old(report).entries@, final(report).entries@,
            NodeId(i as u64), FailedRoundCause::RunFailed),
        reply_family_values::finish(0, result) == retries_facts::failed_run(n),
        reply_family_values::callable(result),
        match result { ReplyChange::Done { note, .. } => note.is_none(),
            ReplyChange::Ask { .. } => false, ReplyChange::Incident { .. } => true },
] {
    kernel_snapshot! { before_report = report.entries@ }
    report.record_failed_round(NodeId(i as u64), FailedRoundCause::RunFailed);
    kernel_proof! {
        report::lemmas::read_failed_round(before_report@, report.entries@,
            NodeId(i as u64), FailedRoundCause::RunFailed);
    }
    run_failed_value(n)
}
}

kernel_fn! {
/// Node-only work after the failed-round report entry.
pub(super) fn run_failed_value(n: Node) -> (result: ReplyChange) [
    requires n.generation as int + 2 <= u64::MAX as int, n.run_failures <= 1, n.run_incidents <= 2,
    ensures reply_family_values::finish(0, result) == retries_facts::failed_run(n),
        reply_family_values::callable(result),
        match result { ReplyChange::Done { note, .. } => note.is_none(),
            ReplyChange::Ask { .. } => false, ReplyChange::Incident { .. } => true },
] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    abandon_operation(&mut node);
    node.pos = operation_start(node.phase);
    node.generation += 1;
    node.run_failures += 1;
    if node.run_failures >= 2 {
        // Every second failure raises an incident. Only Rob can clear the second one.
        node.run_failures = 0;
        node.run_incidents = (node.run_incidents + 1).min(2);
        let needs_rob = node.run_incidents >= 2;
        kernel_proof! { retries_facts::incident(n, node, needs_rob); }
        return ReplyChange::Incident { node, needs_rob };
    }
    kernel_proof! { retries_facts::restarted(n, node); }
    ReplyChange::Done { node, note }
}
}

kernel_fn! {
/// Canonical event-independent failed round node work.
pub(super) fn failed_round_node(n: Node, i: usize, fast: bool, nq: u64, step: ReviewStop, report: &mut Report) -> (result: ReplyChange) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures report::failed_round(old(report).entries@, final(report).entries@,
            NodeId(i as u64), report::failure_cause(step)),
        result == reply_family_values::failed_round(n, fast, step),
        reply_family_values::retry_bound(n, nq, result),
        !(n.phase is Closed) ==> transition_safety::node_step(n, reply_family_values::finish(nq, result)),

] proof [reveal(retirement::stopped); reveal(retirement::incident);] {
    kernel_snapshot! { before_report = report.entries@ }
    let mut node = n;
    let mut note: Option<Notice> = None;
kernel_proof! { report::lemmas::failure_kind(step); }
    let cause = match step {
        ReviewStop::ChecksFailed => FailedRoundCause::ChecksFailed,
        ReviewStop::ChangesRequested => FailedRoundCause::ChangesRequested,
        ReviewStop::CiFailed => FailedRoundCause::CiFailed,
        _ => FailedRoundCause::MakerCantMeet,
    };
    if step != ReviewStop::MakerCantMeet || node.round == 0 {
        report.record_failed_round(NodeId(i as u64), cause);
        kernel_proof! { report::lemmas::read_failed_round(before_report@, report.entries@,
            NodeId(i as u64), cause); }

    }
    let m = &mut node;
    if m.round == 0 {
        // The fix round resumes the maker's session. It's free.
        m.round = 1;
        set_phase(m, if m.holds { Phase::Making } else { Phase::Ready(Resume::Making) });

    } else {

        kernel_snapshot! { before_rung_report = report.entries@ }
        let result = next_rung_node(node, i, fast, nq, step, report);
        kernel_proof! {
            report::lemmas::round_delta(before_rung_report@, report.entries@, NodeId(i as u64),
                if step == ReviewStop::MakerCantMeet { Some(FailedRoundCause::MakerCantMeet) } else { None });
            report::lemmas::append_failed_round(before_report@, report.entries@, NodeId(i as u64), cause);
        }
        return result;

    }
    kernel_proof! {
        report::lemmas::read_failed_round(before_report@, report.entries@, NodeId(i as u64), cause);
        report::lemmas::append_failed_round(before_report@, report.entries@, NodeId(i as u64), cause);
    }
    ReplyChange::Done { node, note }
}
}

kernel_fn! {
/// Canonical event-independent next rung node work.
pub(super) fn next_rung_node(n: Node, i: usize, fast: bool, nq: u64, step: ReviewStop, report: &mut Report) -> (result: ReplyChange) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures report::forwards(old(report).entries@, final(report).entries@,
            report::round_entries(NodeId(i as u64),
                if step == ReviewStop::MakerCantMeet { Some(FailedRoundCause::MakerCantMeet) }
                else { None })),
        result == reply_family_values::rung(n, fast, step),
        reply_family_values::retry_bound(n, nq, result),
        !(n.phase is Closed) ==> transition_safety::node_step(n, reply_family_values::finish(nq, result)),

] proof [reveal(retirement::stopped); reveal(retirement::incident);] {
    kernel_snapshot! { before_report = report.entries@ }
    let mut node = n;
    let mut note: Option<Notice> = None;
if step == ReviewStop::MakerCantMeet {
        report.record_failed_round(NodeId(i as u64), FailedRoundCause::MakerCantMeet);

    }

    kernel_proof! {
        if step == ReviewStop::MakerCantMeet {
            report::lemmas::read_failed_round(before_report@, report.entries@, NodeId(i as u64), FailedRoundCause::MakerCantMeet);
        }
        report::lemmas::forward_round(before_report@, report.entries@, NodeId(i as u64),
            if step == ReviewStop::MakerCantMeet { Some(FailedRoundCause::MakerCantMeet) } else { None });
    }
    let fail_fast = fast;
    let m = &mut node;
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
            return ReplyChange::Ask { node, kind: QKind::OutOfCredit, cont: Cont::Ready(Resume::Making), cost: 1, };
        }
    } else if !m.replanned {
        m.replanned = true;
        m.holds = false;
        if m.credit.can_spend(1) {
            m.credit.spend(1);
            m.plan_retries = 0;
            set_phase(m, Phase::Planning { must_split: false });
            note = Some(Notice::Note(NoteKind::Replanned));
        } else {
            return ReplyChange::Ask { node, kind: QKind::OutOfCredit, cont: Cont::Planning { must_split: false }, cost: 1, };
        }
    } else {
        return ReplyChange::Ask { node, kind: QKind::CantMeet, cont: Cont::Ready(Resume::Making), cost: 1 };
    }
    ReplyChange::Done { node, note }
}
}

kernel_fn! {
/// Canonical event-independent out of room node work.
pub(super) fn out_of_room_node(n: Node, fast: bool, nq: u64, step: ReviewStop) -> (result: ReplyChange) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures result == reply_family_values::room(n, fast, step),
        reply_family_values::callable(result),
] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    let fail_fast = fast;
    if fail_fast {
        node = stop_for_review_v(node, step);
        kernel_proof! { reveal(reply_family_values::room); }
        return ReplyChange::Done { node, note };
    }
    node.holds = false;
    if node.credit.can_spend(1) {
        node.credit.spend(1);
        node.plan_retries = 0;
        node = set_phase_v(node, Phase::Planning { must_split: true });
    } else {
        kernel_proof! { reveal(reply_family_values::room); }
        return ReplyChange::Ask { node, kind: QKind::OutOfCredit, cont: Cont::Planning { must_split: true }, cost: 1, };
    }
    kernel_proof! { reveal(reply_family_values::room); }
    ReplyChange::Done { node, note }
}
}

kernel_fn! {
/// Canonical event-independent plan retry node work.
pub(super) fn plan_retry_node(n: Node, fast: bool, nq: u64, cant_meet: bool) -> (result: ReplyChange) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures result == reply_family_values::plan_retry(n, fast, cant_meet),
        reply_family_values::retry_bound(n, nq, result),
        fast && cant_meet && n.phase is Planning ==>
            reply_family_values::finish(nq, result).phase == Phase::Stopping(Outcome::Stopped)
                && reply_family_values::finish(nq, result).review_stop == Some(ReviewStop::PlanCantMeet),
        !(n.phase is Closed) ==> transition_safety::node_step(n, reply_family_values::finish(nq, result)),

] proof [reveal(reply_family_values::plan_retry); reveal(retirement::stopped); reveal(retirement::incident);] {
    let mut node = n;
    let mut note: Option<Notice> = None;
let fail_fast = fast;
    let m = &mut node;
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
        return ReplyChange::Ask { node, kind: QKind::CantMeet, cont: Cont::Planning { must_split }, cost: 1 };
    }
    ReplyChange::Done { node, note }
}
}

kernel_fn! {
/// Canonical event-independent recheck node work.
pub(super) fn recheck_node(n: Node, nq: u64, target: Phase) -> (result: ReplyChange) [
    requires !(target is Closed), node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures result == reply_family_values::recheck(n, target),
        reply_family_values::retry_bound(n, nq, result),
        (ordinary_await(n) && n.phase == Phase::Rebasing && target == Phase::Checking
            ==> distance(reply_family_values::finish(nq, result)) < distance(n)),
        !(n.phase is Closed) ==> transition_safety::node_step(n, reply_family_values::finish(nq, result)),

] proof [reveal(retirement::stopped); reveal(retirement::incident); reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank);] {
    let mut node = n;
    let mut note: Option<Notice> = None;
let m = &mut node;
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

    } else {
        return ReplyChange::Ask { node, kind: QKind::OutOfCredit, cont: Cont::Ready(if target == Phase::Reviewing {
                Resume::Reviewing
            } else {
                Resume::Checking
            }), cost: 1, };

    }
    ReplyChange::Done { node, note }
}
}

kernel_fn! {
/// Value form of the unchanged review-stop transition.
pub(super) fn stop_for_review_v(n: Node, step: ReviewStop) -> (result: Node) [
    requires n.generation < u64::MAX,
    ensures result == retirement::stopped(Node { review_stop: Some(step), ..n }, Outcome::Stopped),
] {
    let mut m = n;
    stop_for_review(&mut m, step);
    m
}
}
