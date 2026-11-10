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
] {
    kernel_snapshot! { before = *w }
    kernel_snapshot! { before_out = *out }
    let change = failed_round_node(w.nodes[i], i, w.fail_fast, w.next_question, step, report); kernel_proof! { reply_family_posts::failed_round_value_posts(w.nodes@[i as int], w.fail_fast, w.next_question, step); }
    resolve_reply_change(w, i, change, out);
    kernel_proof! {
        reply_family_posts::failed_round_posts(before@.nodes@[i as int], before@.fail_fast, before@.next_question, step,
            i as int, before_out@, *out);
        reply_family_posts::retry_lift(before@, *w, i as int, change, before_out@, *out);
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
] {
    kernel_snapshot! { before = *w }
    kernel_snapshot! { before_out = *out }
    let change = next_rung_node(w.nodes[i], i, w.fail_fast, w.next_question, step, report); kernel_proof! { reply_family_posts::rung_value_posts(w.nodes@[i as int], w.fail_fast, w.next_question, step); }
    resolve_reply_change(w, i, change, out);
    kernel_proof! {
        reply_family_posts::rung_posts(before@.nodes@[i as int], before@.fail_fast, before@.next_question, step,
            i as int, before_out@, *out);
        reply_family_posts::retry_lift(before@, *w, i as int, change, before_out@, *out);
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
] {
    kernel_snapshot! { before = *w }
    kernel_snapshot! { before_out = *out }
    let change = plan_retry_node(w.nodes[i], w.fail_fast, w.next_question, cant_meet); kernel_proof! { reply_family_posts::plan_retry_value_posts(w.nodes@[i as int], w.fail_fast, w.next_question, cant_meet); }
    resolve_reply_change(w, i, change, out);
    kernel_proof! {
        reply_family_posts::plan_retry_posts(before@.nodes@[i as int], before@.fail_fast, before@.next_question, cant_meet,
            i as int, before_out@, *out);
        reply_family_posts::retry_lift(before@, *w, i as int, change, before_out@, *out);
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
] {
    kernel_snapshot! { before = *w }
    kernel_snapshot! { before_out = *out }
    let change = recheck_node(w.nodes[i], w.next_question, target); kernel_proof! { reply_family_posts::recheck_value_posts(w.nodes@[i as int], w.next_question, target); }
    resolve_reply_change(w, i, change, out);
    kernel_proof! {
        reply_family_posts::recheck_posts(before@.nodes@[i as int], before@.next_question, target,
            i as int, before_out@, *out);
        reply_family_posts::retry_lift(before@, *w, i as int, change, before_out@, *out);
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
    node = failed_prefix(node);
    kernel_proof! { reply_family_posts::failed_arms(n); }
    if node.run_failures >= 2 {
        return run_failed_incident(node);
    }
    run_failed_restart(node)
}
}

kernel_fn! {
fn run_failed_incident(n: Node) -> (result: ReplyChange) [
    requires n.run_incidents <= 2,
    ensures result == reply_family_values::failed_incident(n),
] {
    let mut node = n;
    // Every second failure raises an incident. Only Rob can clear the second one.
    node.run_failures = 0;
    node.run_incidents = incident_count(node.run_incidents);
    let needs_rob = node.run_incidents >= 2;
    ReplyChange::Incident { node, needs_rob }
}
}

kernel_fn! {
fn run_failed_restart(node: Node) -> (result: ReplyChange) [
    ensures result == reply_family_values::failed_restart(node),
] {
    let note: Option<Notice> = None;
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
        reply_family_values::callable(result),
] {

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

    kernel_proof! { reply_family_posts::failed_round_arms(n, fast, step); }
    if node.round == 0 {
        let result = failed_round_free(node);
        kernel_proof! {
            report::lemmas::read_failed_round(before_report@, report.entries@, NodeId(i as u64), cause);
            report::lemmas::append_failed_round(before_report@, report.entries@, NodeId(i as u64), cause);
            assert(result == reply_family_values::failed_round(n, fast, step));
            assert(reply_family_values::callable(result));
        }
        return result;
    } else {

        kernel_snapshot! { before_rung_report = report.entries@ }
        let result = next_rung_node(node, i, fast, nq, step, report); kernel_proof! { reply_family_posts::rung_value_posts(node, fast, nq, step); }
        kernel_proof! {
            report::lemmas::round_delta(before_rung_report@, report.entries@, NodeId(i as u64),
                if step == ReviewStop::MakerCantMeet { Some(FailedRoundCause::MakerCantMeet) } else { None });
            report::lemmas::append_failed_round(before_report@, report.entries@, NodeId(i as u64), cause);
        }
        kernel_proof! { assert(result == reply_family_values::failed_round(n, fast, step)); assert(reply_family_values::callable(result)); }
        return result;

    }
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
        reply_family_values::callable(result),
] {

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

    kernel_proof! { reply_family_posts::rung_arms(n, fast, step); }
    if fail_fast {
        let result = out_of_room_fast(node, step);
        kernel_proof! {
            assert(result == reply_family_values::rung(n, fast, step));
        }
        return result;
    } else if node.attempt < 3 {
        node = rung_bump_v(node);
        if node.credit.can_spend(1) {
            let result = next_rung_paid(node);
            kernel_proof! {
                assert(result == reply_family_values::rung(n, fast, step));
            }
            return result;
        } else {
            let result = next_rung_ask(node);
            kernel_proof! {
                assert(result == reply_family_values::rung(n, fast, step));
            }
            return result;
        }
    } else if !node.replanned {
        node = rung_replan_v(node);
        if node.credit.can_spend(1) {
            let result = next_rung_replan_paid(node);
            kernel_proof! {
                assert(result == reply_family_values::rung(n, fast, step));
            }
            return result;
        } else {
            let result = next_rung_replan_ask(node);
            kernel_proof! {
                assert(result == reply_family_values::rung(n, fast, step));
            }
            return result;
        }
    } else {
        let result = next_rung_cant_meet(node);
        kernel_proof! {
            assert(result == reply_family_values::rung(n, fast, step));
        }
        return result;
    }
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
    kernel_proof! { reply_family_posts::room_bridge(n, fast, step); }
    if fail_fast {
        let result = out_of_room_fast(node, step);
        kernel_proof! {
            assert(result == reply_family_values::room(n, fast, step));
            assert(reply_family_values::callable(result));
        }
        return result;
    }
    node = unheld_v(node);
    if node.credit.can_spend(1) {
        let result = out_of_room_paid(node);
        kernel_proof! {
            assert(result == reply_family_values::room(n, fast, step));
            assert(reply_family_values::callable(result));
        }
        result
    } else {
        let result = out_of_room_ask(node);
        kernel_proof! {
            assert(result == reply_family_values::room(n, fast, step));
            assert(reply_family_values::callable(result));
        }
        result
    }
}
}

kernel_fn! {
fn out_of_room_fast(n: Node, step: ReviewStop) -> (result: ReplyChange) [
    requires n.generation < u64::MAX,
    ensures result == reply_family_values::room_fast(n, step),
] {
    let mut node = n;
    let note: Option<Notice> = None;
    node = stop_for_review_v(node, step);
    ReplyChange::Done { node, note }
}
}

kernel_fn! {
fn out_of_room_paid(n: Node) -> (result: ReplyChange) [
    requires node_headroom(n),
    ensures result == reply_family_values::room_paid(n),
] {
    let mut node = n;
    let note: Option<Notice> = None;
    node.credit = spend_v(node.credit, 1);
    node.plan_retries = 0;
    node = set_phase_v(node, Phase::Planning { must_split: true });
    ReplyChange::Done { node, note }
}
}

kernel_fn! {
fn out_of_room_ask(node: Node) -> (result: ReplyChange) [
    ensures result == reply_family_values::room_ask(node),
] {
    ReplyChange::Ask { node, kind: QKind::OutOfCredit,
        cont: Cont::Planning { must_split: true }, cost: 1, }
}
}

kernel_fn! {
/// Canonical event-independent plan retry node work.
pub(super) fn plan_retry_node(n: Node, fast: bool, nq: u64, cant_meet: bool) -> (result: ReplyChange) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures result == reply_family_values::plan_retry(n, fast, cant_meet),
        reply_family_values::callable(result),
] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    let fail_fast = fast;
    node = plan_count_v(node);
    let phase = node.phase;
    kernel_proof! { reply_family_posts::plan_retry_arms(n, fast, cant_meet); }
    if fail_fast && cant_meet {
        let result = out_of_room_fast(node, ReviewStop::PlanCantMeet);
        kernel_proof! {
            assert(result == reply_family_values::plan_retry(n, fast, cant_meet));
        }
        return result;
    } else if node.plan_retries < 2 {
        let result = plan_retry_again(node);
        kernel_proof! {
            assert(result == reply_family_values::plan_retry(n, fast, cant_meet));
        }
        return result;
    } else {
        let result = plan_retry_ask(node);
        kernel_proof! {
            assert(result == reply_family_values::plan_retry(n, fast, cant_meet));
        }
        return result;
    }
}
}

kernel_fn! {
/// Canonical event-independent recheck node work.
pub(super) fn recheck_node(n: Node, nq: u64, target: Phase) -> (result: ReplyChange) [
    requires !(target is Closed), node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures result == reply_family_values::recheck(n, target),
        reply_family_values::callable(result),
] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    node = recheck_count_v(node);
    let free = node.rechecks <= FREE_RECHECKS;
    kernel_proof! { reply_family_posts::recheck_arms(n, target); }
    if free || node.credit.can_spend(1) {
        if !free {
            let result = recheck_paid(node, target);
            kernel_proof! {
                assert(result == reply_family_values::recheck(n, target));
            }
            return result;
        } else {
            let result = recheck_free(node, target);
            kernel_proof! {
                assert(result == reply_family_values::recheck(n, target));
            }
            return result;
        }
    } else {
        let result = recheck_ask(node, target);
        kernel_proof! {
            assert(result == reply_family_values::recheck(n, target));
        }
        return result;
    }
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

kernel_fn! {
/// Return the unchanged credit spend as a whole value for exact node equations.
pub(super) fn spend_v(c: Credit, cost: u64) -> (result: Credit) [
    requires c.spent as int + cost as int <= u64::MAX as int,
    ensures result == (Credit { spent: (c.spent + cost) as u64, ..c }),
] {
    let mut m = c;
    m.spend(cost);
    m
}
}

kernel_fn! {
/// The original failed-run prefix, returned with its whole-node equation.
fn failed_prefix(n: Node) -> (result: Node) [
    requires n.generation < u64::MAX, n.run_failures < u8::MAX,
    ensures result == (Node { pos: if n.phase == Phase::Making { 1u8 } else { 0u8 },
        generation: (n.generation + 1) as u64,
        run_failures: (n.run_failures + 1) as u8, ..retries_facts::abandoned(n) }),
] {
    let mut m = n;
    abandon_operation(&mut m);
    m.pos = operation_start(m.phase);
    m.generation += 1;
    m.run_failures += 1;
    m
}
}

kernel_fn! {
/// The original saturated incident count, with its exact result equation.
fn incident_count(c: u8) -> (result: u8) [
    requires c <= 2,
    ensures result == (if c < 2 { (c + 1) as u8 } else { 2u8 }),
] {
    (c + 1).min(2)
}
}

kernel_fn! {
fn next_rung_paid(n: Node) -> (result: ReplyChange) [
    requires node_headroom(n),
    ensures result == reply_family_values::rung_paid(n), reply_family_values::callable(result),
] proof [reveal(reply_family_values::rung_paid);] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    node.credit = spend_v(node.credit, 0);
    node = set_phase_v(node, if node.holds { Phase::Making } else { Phase::Ready(Resume::Making) });
    ReplyChange::Done { node, note }
}
}

kernel_fn! {
fn next_rung_ask(node: Node) -> (result: ReplyChange) [
    requires node.generation < u64::MAX,
    ensures result == reply_family_values::rung_ask(node), reply_family_values::callable(result),
] proof [reveal(reply_family_values::rung_ask);] {
    ReplyChange::Ask { node, kind: QKind::OutOfCredit, cont: Cont::Ready(Resume::Making), cost: 1, }
}
}

kernel_fn! {
fn next_rung_replan_paid(n: Node) -> (result: ReplyChange) [
    requires node_headroom(n),
    ensures result == reply_family_values::rung_replan_paid(n), reply_family_values::callable(result),
] proof [reveal(reply_family_values::rung_replan_paid);] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    node.credit = spend_v(node.credit, 1);
    node.plan_retries = 0;
    node = set_phase_v(node, Phase::Planning { must_split: false });
    note = Some(Notice::Note(NoteKind::Replanned));
    ReplyChange::Done { node, note }
}
}

kernel_fn! {
fn next_rung_replan_ask(node: Node) -> (result: ReplyChange) [
    requires node.generation < u64::MAX,
    ensures result == reply_family_values::rung_replan_ask(node), reply_family_values::callable(result),
] proof [reveal(reply_family_values::rung_replan_ask);] {
    ReplyChange::Ask { node, kind: QKind::OutOfCredit, cont: Cont::Planning { must_split: false }, cost: 1, }
}
}

kernel_fn! {
fn next_rung_cant_meet(node: Node) -> (result: ReplyChange) [
    requires node.generation < u64::MAX,
    ensures result == reply_family_values::rung_cant_meet(node), reply_family_values::callable(result),
] proof [reveal(reply_family_values::rung_cant_meet);] {
    ReplyChange::Ask { node, kind: QKind::CantMeet, cont: Cont::Ready(Resume::Making), cost: 1 }
}
}

kernel_fn! {
fn failed_round_free(n: Node) -> (result: ReplyChange) [
    requires node_headroom(n),
    ensures result == reply_family_values::round_free(n), reply_family_values::callable(result),
] proof [reveal(reply_family_values::round_free);] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    node.round = 1;
    node = set_phase_v(node, if node.holds { Phase::Making } else { Phase::Ready(Resume::Making) });
    ReplyChange::Done { node, note }
}
}

kernel_fn! {
fn plan_retry_again(n: Node) -> (result: ReplyChange) [
    requires node_headroom(n),
    ensures result == reply_family_values::plan_again(n), reply_family_values::callable(result),
] proof [reveal(reply_family_values::plan_again);] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    let phase = node.phase;
    node = set_phase_v(node, phase);
    ReplyChange::Done { node, note }
}
}

kernel_fn! {
fn plan_retry_ask(n: Node) -> (result: ReplyChange) [
    requires n.generation < u64::MAX,
    ensures result == reply_family_values::plan_ask(n), reply_family_values::callable(result),
] proof [reveal(reply_family_values::plan_ask);] {
    let mut node = n;
    let phase = node.phase;
    node.plan_retries = 0;
    let must_split = matches!(phase, Phase::Planning { must_split: true });
    ReplyChange::Ask { node, kind: QKind::CantMeet, cont: Cont::Planning { must_split }, cost: 1 }
}
}

kernel_fn! {
fn recheck_free(n: Node, target: Phase) -> (result: ReplyChange) [
    requires node_headroom(n),
    ensures result == reply_family_values::recheck_free(n, target), reply_family_values::callable(result),
] proof [reveal(reply_family_values::recheck_free);] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    node = held_v(node, target);
    node = set_phase_v(node, target);
    ReplyChange::Done { node, note }
}
}

kernel_fn! {
fn recheck_paid(n: Node, target: Phase) -> (result: ReplyChange) [
    requires node_headroom(n),
    ensures result == reply_family_values::recheck_paid(n, target), reply_family_values::callable(result),
] proof [reveal(reply_family_values::recheck_paid);] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    node = spent_v(node);
    node = held_v(node, target);
    node = set_phase_v(node, target);
    ReplyChange::Done { node, note }
}
}

kernel_fn! {
fn recheck_ask(node: Node, target: Phase) -> (result: ReplyChange) [
    requires node.generation < u64::MAX,
    ensures result == reply_family_values::recheck_ask(node, target), reply_family_values::callable(result),
] proof [reveal(reply_family_values::recheck_ask);] {
    ReplyChange::Ask { node, kind: QKind::OutOfCredit, cont: Cont::Ready(if target == Phase::Reviewing {
                Resume::Reviewing
            } else {
                Resume::Checking
            }), cost: 1, }
}
}

kernel_fn! {
fn retry_count(c: u8) -> (result: u8) [
    ensures result == (if c < 255 { (c + 1) as u8 } else { 255u8 }),
] { c.saturating_add(1) }
}

kernel_fn! {
fn rung_bump_v(n: Node) -> (result: Node) [
    requires n.attempt < 3,
    ensures result == reply_family_values::bumped(n),
        result.credit == n.credit, result.generation == n.generation,
        result.next_seq == n.next_seq,
] proof [reveal(reply_family_values::bumped);] {
    let mut m = n;
    m.attempt += 1; m.review_restarted = false; m.round = 0;
    m
}
}

kernel_fn! {
fn rung_replan_v(n: Node) -> (result: Node) [
    ensures result == reply_family_values::replanning(n),
        result.credit == n.credit, result.generation == n.generation,
        result.next_seq == n.next_seq,
] proof [reveal(reply_family_values::replanning);] {
    let mut m = n;
    m.replanned = true; m.holds = false;
    m
}
}

kernel_fn! {
fn recheck_count_v(n: Node) -> (result: Node) [
    ensures result == reply_family_values::rechecked(n),
        result.credit == n.credit, result.generation == n.generation,
        result.next_seq == n.next_seq,
] proof [reveal(reply_family_values::rechecked);] {
    let mut m = n;
    m.rechecks = retry_count(m.rechecks);
    m
}
}

kernel_fn! {
fn plan_count_v(n: Node) -> (result: Node) [
    ensures result == reply_family_values::plan_counted(n),
        result.credit == n.credit, result.generation == n.generation,
        result.next_seq == n.next_seq,
] proof [reveal(reply_family_values::plan_counted);] {
    let mut m = n;
    m.plan_retries = retry_count(m.plan_retries);
    m
}
}

kernel_fn! {
fn held_v(n: Node, target: Phase) -> (result: Node) [
    ensures result == reply_family_values::held(n, target),
        result.credit == n.credit, result.generation == n.generation,
        result.next_seq == n.next_seq,
] proof [reveal(reply_family_values::held);] {
    let mut m = n;
    if !target.holding() { m.holds = false; }
    m
}
}

kernel_fn! {
fn unheld_v(n: Node) -> (result: Node) [
    ensures result == reply_family_values::unheld(n),
        result.credit == n.credit, result.generation == n.generation,
        result.next_seq == n.next_seq,
] proof [reveal(reply_family_values::unheld);] {
    let mut m = n;
    m.holds = false;
    m
}
}

kernel_fn! {
/// Spend one credit unit and return the whole updated node before recheck dispatch.
fn spent_v(n: Node) -> (result: Node) [
    requires n.credit.spent < u64::MAX,
    ensures result == reply_family_values::spent(n),
        result.generation == n.generation, result.next_seq == n.next_seq,
] {
    let mut m = n;
    m.credit = spend_v(m.credit, 1);
    m
}
}
