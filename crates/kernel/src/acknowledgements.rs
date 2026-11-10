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
    ensures (node_checked(*old(w), old(w).nodes@[i as int])
            && (!old(w).nodes@[i as int].paused || old(w).nodes@[i as int].phase is Stopping)
            && (match old(w).nodes@[i as int].effect {
                Some(e) => e.seq == seq && (if e.kind == EffectKind::Enqueue {
                    ack == Ack::DeliveryAccepted
                } else { ack == Ack::Confirmed }), None => false,
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
        on_ack_post(*old(w), *final(w), i as int, seq, ack, *old(out), *final(out)),
] proof [reveal(on_ack_post);] {
    let Some(e) = w.nodes[i].effect else { return };
    if e.seq != seq {
        return;
    }
    if ack == Ack::DeliveryAccepted && e.kind != EffectKind::Enqueue {
        return;
    }
    if !e.cancelling && e.kind == EffectKind::Enqueue {
        if ack == Ack::Confirmed {
            return;
        }
        if ack == Ack::DeliveryAccepted
            && (w.nodes[i].phase != Phase::Publishing
                || w.nodes[i].current() != Some(Item::Do(EffectKind::Enqueue)))
        {
            return;
        }
    }
    ack_live(w, i, e, ack, out);
}
}

kernel_fn! {
/// Acknowledgement rows / #771 / K5 G4: the live part of on_ack after its guards,
/// moved verbatim. It carries on_ack's contract for an admitted ack, plus the exact
/// per-path node and output facts the Ack family arm consumes.
fn ack_live(w: &mut World, i: usize, e: InFlight, ack: Ack,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        old(w).nodes@[i as int].effect == Some(e),
        ack == Ack::DeliveryAccepted ==> e.kind == EffectKind::Enqueue,
        !e.cancelling && e.kind == EffectKind::Enqueue ==> ack != Ack::Confirmed
            && (ack == Ack::DeliveryAccepted ==> old(w).nodes@[i as int].phase == Phase::Publishing
                && current_item(old(w).nodes@[i as int]) == Some(Item::Do(EffectKind::Enqueue))),
    ensures (node_checked(*old(w), old(w).nodes@[i as int])
            && (!old(w).nodes@[i as int].paused || old(w).nodes@[i as int].phase is Stopping)
            && (if e.kind == EffectKind::Enqueue { ack == Ack::DeliveryAccepted } else { ack == Ack::Confirmed })
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
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
        ack_live_post(old(w).nodes@[i as int], final(w).nodes@[i as int], e, ack,
            old(w).next_question, final(w).next_question, i as int, *old(out), *final(out)),
] proof [reveal(distance);] {
    kernel_snapshot! { ack_node = w.nodes@[i as int] }
    kernel_snapshot! { ack_question = w.next_question }
    kernel_snapshot! { ack_out = *out }
    w.nodes[i].effect = None;
    if e.cancelling {
        kernel_proof! { ack_cancel_post(ack_node@, w.nodes@[i as int], e, ack, ack_question@, w.next_question, i as int, ack_out@, *out); }
        return;
    }
    match ack {
        Ack::Confirmed => {
            confirmed(&mut w.nodes[i], e.kind);
            kernel_proof! { ack_confirmed_post(ack_node@, w.nodes@[i as int], e, ack, ack_question@, w.next_question, i as int, ack_out@, *out); }
        },
        Ack::DeliveryAccepted => {
            delivery_accepted(&mut w.nodes[i]);
            kernel_proof! { ack_delivery_post(ack_node@, w.nodes@[i as int], e, ack, ack_question@, w.next_question, i as int, ack_out@, *out); }
        },
        Ack::Retryable => {
            delivery_failed(w, i, e.kind, out);
            kernel_proof! { ack_retry_post(ack_node@, w.nodes@[i as int], e, ack, ack_question@, w.next_question, i as int, ack_out@, *out); }
        },
        Ack::Permanent => {
            effect_failed(w, i, e.kind, out);
            kernel_proof! { ack_permanent_post(ack_node@, w.nodes@[i as int], e, ack, ack_question@, w.next_question, i as int, ack_out@, *out); }
        },
        Ack::BodyPolicyRefused if e.kind == EffectKind::OpenPr => {
            stop_for_review(&mut w.nodes[i], ReviewStop::PublicationBody);
            kernel_proof! { ack_review_post(ack_node@, w.nodes@[i as int], e, ack, ack_question@, w.next_question, i as int, ack_out@, *out); }
        },
        Ack::BodyPolicyRefused => {
            effect_failed(w, i, e.kind, out);
            kernel_proof! { ack_permanent_post(ack_node@, w.nodes@[i as int], e, ack, ack_question@, w.next_question, i as int, ack_out@, *out); }
        },
    }
}
}

kernel_fn! {
/// PRD #878 / TD acceptance: release ownership and preserve approved delivery evidence.
fn delivery_accepted(m: &mut Node) -> (result: ()) [
    requires old(m).generation < u64::MAX, old(m).effect.is_none(),
    ensures final(m).phase == Phase::AwaitingDelivery,
        final(m).generation as int == old(m).generation as int + 1,
        final(m).pos == 0, !final(m).holds, final(m).ticket.is_none(),
        final(m).run.is_none(), final(m).job.is_none(), !final(m).pool,
        final(m).effect.is_none(), final(m).question.is_none(),
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        final(m).head == old(m).head, final(m).checked == old(m).checked,
        final(m).approved == old(m).approved, final(m).proposed == old(m).proposed,
        final(m).adopted == old(m).adopted, final(m).worktree == old(m).worktree,
        final(m).pr_open == old(m).pr_open, final(m).had_pr == old(m).had_pr,
        final(m).merged == old(m).merged, final(m).credit == old(m).credit,
        final(m).attempt == old(m).attempt, final(m).round == old(m).round,
        final(m).replanned == old(m).replanned, final(m).plan_retries == old(m).plan_retries,
        final(m).rechecks == old(m).rechecks, final(m).review_restarted == old(m).review_restarted,
        final(m).paused == old(m).paused, final(m).waiting_incident == old(m).waiting_incident,
        final(m).incident_needs_rob == old(m).incident_needs_rob,
        final(m).next_seq == old(m).next_seq, final(m).last_cmd == old(m).last_cmd,
        final(m).fx_failures == 0, final(m).fx_incidents == 0,
        settle_shape(*old(m)) ==> settle_shape(*final(m)),
        old(m).phase == Phase::Publishing ==> distance(*final(m)) + 2 <= distance(*old(m)),
        final(m).review_stop == old(m).review_stop,
] proof [reveal(distance);] {
    m.fx_failures = 0;
    m.fx_incidents = 0;
    m.holds = false;
    m.run = None;
    m.job = None;
    m.pool = false;
    m.question = None;
    set_phase(m, Phase::AwaitingDelivery);
}
}

kernel_fn! {
/// Confirmed-effect rows / #771: at most one adoption generation is allocated.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn confirmed(m: &mut Node, kind: EffectKind) -> (result: ()) [
    requires old(m).generation < u64::MAX,
    ensures (matches!(kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
            ==> distance(*final(m)) == distance(*old(m))),
        (old(m).effect.is_none() && old(m).question.is_none() && !old(m).waiting_incident
            && (!old(m).paused || old(m).phase is Stopping)
            && kind != EffectKind::Enqueue
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
        *final(m) == agenda_spec(resources_spec(*old(m), kind), kind),
] {
    kernel_snapshot! { before_confirmed = *m }
    confirmed_resources(m, kind);
    kernel_proof! { cleanup_lemmas::resource_distance(before_confirmed@, *m); }
    confirmed_agenda(m, kind);
}
}

kernel_fn! {
/// Confirmed-effect rows / #771: release the confirmed resource and reset the
/// failure ladder, framed exactly so agenda advancement sees one node value.
fn confirmed_resources(m: &mut Node, kind: EffectKind) -> (result: ()) [
    ensures *final(m) == (Node {
            fx_failures: 0, fx_incidents: 0,
            run: if kind == EffectKind::StopRun { None } else { old(m).run },
            pool: if kind == EffectKind::StopRun { false } else { old(m).pool },
            job: if kind == EffectKind::WithdrawJob { None } else { old(m).job },
            pr_open: if kind == EffectKind::ClosePr { false } else { old(m).pr_open },
            worktree: if kind == EffectKind::RemoveWorktree { false } else { old(m).worktree },
            ..*old(m)
        }),
] {
    m.fx_failures = 0;
    m.fx_incidents = 0;
    release_confirmed(m, kind);
}
}

kernel_fn! {
/// Confirmed-effect rows / #771: release only the resource the effect confirmed.
fn release_confirmed(m: &mut Node, kind: EffectKind) -> (result: ()) [
    ensures *final(m) == (Node {
            run: if kind == EffectKind::StopRun { None } else { old(m).run },
            pool: if kind == EffectKind::StopRun { false } else { old(m).pool },
            job: if kind == EffectKind::WithdrawJob { None } else { old(m).job },
            pr_open: if kind == EffectKind::ClosePr { false } else { old(m).pr_open },
            worktree: if kind == EffectKind::RemoveWorktree { false } else { old(m).worktree },
            ..*old(m)
        }),
] {
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
}
}

kernel_fn! {
/// Confirmed-effect rows / #771: after resource cleanup, consume the same
/// matching agenda item or enter Checking for an adopted worktree.
// The pinned Verus frontend requires nested if-let instead of let chains.
#[allow(clippy::collapsible_if)]
fn confirmed_agenda(m: &mut Node, kind: EffectKind) -> (result: ()) [
    requires old(m).generation < u64::MAX,
        kind == EffectKind::StopRun ==> !old(m).pool,
    ensures (matches!(kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
            ==> distance(*final(m)) == distance(*old(m))),
        (old(m).effect.is_none() && old(m).question.is_none() && !old(m).waiting_incident
            && (!old(m).paused || old(m).phase is Stopping)
            && kind != EffectKind::Enqueue
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
        *final(m) == agenda_spec(*old(m), kind),
] {
    *m = confirmed_agenda_value(*m, kind);
}
}

kernel_fn! {
fn confirmed_agenda_adopt(n: Node, head: HeadId) -> (result: Node) [
    requires n.generation < u64::MAX,
    ensures result == agenda_adopt_spec(n, head),
] {
    let m = Node { adopted: true, head: Some(head), checked: None, ..n };
    Node { generation: m.generation + 1, phase: Phase::Checking, pos: 0, ticket: None,
        effect: match m.effect {
            Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
        }, ..m }
}
}

kernel_fn! {
fn confirmed_agenda_step(n: Node) -> (result: Node) [
    requires n.pos < u8::MAX,
    ensures result == agenda_step_spec(n),
] {
    Node { pos: n.pos + 1, ..n }
}
}

kernel_fn! {
fn confirmed_agenda_idle(n: Node) -> (result: Node) [
    ensures result == agenda_idle_spec(n),
] {
    n
}
}

kernel_fn! {
fn confirmed_agenda_value(n: Node, kind: EffectKind) -> (result: Node) [
    requires n.generation < u64::MAX,
        kind == EffectKind::StopRun ==> !n.pool,
    ensures (matches!(kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
            ==> distance(result) == distance(n)),
        (n.effect.is_none() && n.question.is_none() && !n.waiting_incident
            && (!n.paused || n.phase is Stopping)
            && kind != EffectKind::Enqueue
            && (current_item(n) == Some(Item::Do(kind))
                || current_item(n) == Some(Item::Teardown(kind)))
            ==> distance(result) + 2 <= distance(n)),
        result.repo == n.repo, result.footprint == n.footprint,
        (settle_shape(n)
            && (n.phase is Stopping ==> current_item(n) == Some(Item::Teardown(kind)))
            ==> settle_shape(result)),
        result.generation as int <= n.generation as int + 1,
        result.generation >= n.generation,
        result.next_seq == n.next_seq, result.credit == n.credit,
        result.question == n.question,
        result.fx_failures == n.fx_failures,
        result.fx_incidents == n.fx_incidents,
        result.last_cmd == n.last_cmd,
        result == agenda_spec(n, kind),
] {
    let result = if kind != EffectKind::Enqueue && !kind.starts_operation() {
        if let Some(Item::Do(k) | Item::Teardown(k)) = n.current() {
            if k == kind {
                if kind == EffectKind::PrepareWorktree && !n.adopted {
                    if let Some(head) = n.proposed {
                        let result = confirmed_agenda_adopt(n, head);
                        kernel_proof! { agenda_result_properties(n, result, kind); }
                        return result;
                    }
                }
                confirmed_agenda_step(n)
            } else { confirmed_agenda_idle(n) }
        } else { confirmed_agenda_idle(n) }
    } else { confirmed_agenda_idle(n) };
    kernel_proof! { agenda_result_properties(n, result, kind); }
    result
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
        final(w).nodes@[i as int].proposed == old(w).nodes@[i as int].proposed,
        final(w).nodes@[i as int].adopted == old(w).nodes@[i as int].adopted,
        final(w).nodes@[i as int].review_stop == old(w).nodes@[i as int].review_stop,
        final(w).nodes@[i as int].attempt == old(w).nodes@[i as int].attempt,
        final(w).nodes@[i as int].replanned == old(w).nodes@[i as int].replanned,
        final(w).nodes@[i as int].approved == old(w).nodes@[i as int].approved,
        final(w).nodes@[i as int].checked == old(w).nodes@[i as int].checked,
        (final(w).nodes@[i as int].phase is Planning ==> old(w).nodes@[i as int].phase is Planning),
        final(w).nodes@[i as int].next_seq >= old(w).nodes@[i as int].next_seq,
        final(out).len == old(out).len + 1,
        forall|k: int| 0 <= k < old(out).len ==> #[trigger] final(out).items@[k] == old(out).items@[k],
        match final(out).items@[old(out).len as int] {
            Some(Out::Effect { node, seq, kind: k, head }) => node == NodeId(i as u64)
                && seq == old(w).nodes@[i as int].next_seq && k == kind
                && head == effect_head(kind, old(w).nodes@[i as int].head)
                && seq < final(w).nodes@[i as int].next_seq,
            Some(Out::Notice { node, .. }) => node == NodeId(i as u64),
            None => false,
        },
] proof [reveal(retirement::incident);] {
    match failed_delivery_node(&mut w.nodes[i], kind) {
        None => emit(w, i, kind, out),
        Some(needs_rob) => raise_incident(w, i, needs_rob, out),
    }
}
}

kernel_fn! {
/// Failed-delivery rows / #771: reset the retry ladder before emitting or
/// raising its incident. This node-local part allocates at most one generation.
fn failed_delivery_node(m: &mut Node, kind: EffectKind) -> (result: Option<bool>) [
    requires old(m).generation < u64::MAX,
        old(m).fx_failures <= 1, old(m).fx_incidents <= 2,
    ensures final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).question == old(m).question, final(m).last_cmd == old(m).last_cmd,
        final(m).fx_failures <= 1, final(m).fx_incidents <= 2,
        result.is_none() == (old(m).fx_failures == 0),
        final(m).proposed == old(m).proposed, final(m).adopted == old(m).adopted,
        final(m).review_stop == old(m).review_stop, final(m).attempt == old(m).attempt,
        final(m).replanned == old(m).replanned, final(m).phase == old(m).phase,
        final(m).head == old(m).head, final(m).approved == old(m).approved,
        final(m).checked == old(m).checked,
] {
    m.fx_failures += 1;
    if m.fx_failures < 2 {
        return None;
    }
    m.fx_failures = 0;
    m.fx_incidents = (m.fx_incidents + 1).min(2);
    let needs_rob = m.fx_incidents >= 2;
    if kind.starts_operation() {
        abandon_operation(m);
        m.pos = operation_start(m.phase);
        m.generation += 1;
    }
    Some(needs_rob)
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
        final(w).nodes@[i as int] == failed_spec(old(w).nodes@[i as int], kind, old(w).next_question),
        final(out).items@ == old(out).items@.update(old(out).len as int,
            Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Incident { needs_rob: true } }))
            .update(old(out).len as int + 1,
                Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Ask { question: old(w).next_question } })),
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

#[cfg(verus_keep_ghost)]
verus! {
/// K5 G4 / I-D2: the head an effect of `kind` carries (emit_node's selection).
pub(super) open spec fn effect_head(kind: EffectKind, head: Option<HeadId>) -> Option<HeadId> {
    match kind {
        EffectKind::Push | EffectKind::PostApproval | EffectKind::OpenPr | EffectKind::Enqueue => head,
        _ => None,
    }
}

/// K5 I-D3: confirmed_resources' exact result.
pub(super) open spec fn resources_spec(m: Node, kind: EffectKind) -> Node {
    Node {
        fx_failures: 0, fx_incidents: 0,
        run: if kind == EffectKind::StopRun { None } else { m.run },
        pool: if kind == EffectKind::StopRun { false } else { m.pool },
        job: if kind == EffectKind::WithdrawJob { None } else { m.job },
        pr_open: if kind == EffectKind::ClosePr { false } else { m.pr_open },
        worktree: if kind == EffectKind::RemoveWorktree { false } else { m.worktree },
        ..m
    }
}

/// K5 I-D3: confirmed_agenda's exact result, including adoption into Checking.
pub(super) open spec fn agenda_spec(m: Node, kind: EffectKind) -> Node {
    if kind != EffectKind::Enqueue
        && !matches!(kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
        && (current_item(m) == Some(Item::Do(kind)) || current_item(m) == Some(Item::Teardown(kind))) {
        if kind == EffectKind::PrepareWorktree && !m.adopted && m.proposed.is_some() {
            agenda_adopt_spec(m, m.proposed.unwrap())
        } else {
            agenda_step_spec(m)
        }
    } else {
        agenda_idle_spec(m)
    }
}

pub(super) open spec fn agenda_adopt_spec(n: Node, head: HeadId) -> Node {
    retirement::phased(Node { adopted: true, head: Some(head), checked: None, ..n }, Phase::Checking)
}
pub(super) open spec fn agenda_step_spec(n: Node) -> Node {
    Node { pos: (n.pos + 1) as u8, ..n }
}
pub(super) open spec fn agenda_idle_spec(n: Node) -> Node { n }

proof fn agenda_result_properties(n: Node, result: Node, kind: EffectKind)

    requires n.generation < u64::MAX,
        kind == EffectKind::StopRun ==> !n.pool,
        result == agenda_spec(n, kind),
    ensures (matches!(kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase)
            ==> distance(result) == distance(n)),
        (n.effect.is_none() && n.question.is_none() && !n.waiting_incident
            && (!n.paused || n.phase is Stopping)
            && kind != EffectKind::Enqueue
            && (current_item(n) == Some(Item::Do(kind))
                || current_item(n) == Some(Item::Teardown(kind)))
            ==> distance(result) + 2 <= distance(n)),
        result.repo == n.repo, result.footprint == n.footprint,
        (settle_shape(n)
            && (n.phase is Stopping ==> current_item(n) == Some(Item::Teardown(kind)))
            ==> settle_shape(result)),
        result.generation as int <= n.generation as int + 1,
        result.generation >= n.generation,
        result.next_seq == n.next_seq, result.credit == n.credit,
        result.question == n.question,
        result.fx_failures == n.fx_failures,
        result.fx_incidents == n.fx_incidents,
        result.last_cmd == n.last_cmd,
        result == agenda_spec(n, kind),
{
    reveal(distance);
}

/// K5 G4 / H29.2: effect_failed's exact node result. `c` already has no effect.
pub(super) open spec fn failed_spec(c: Node, kind: EffectKind, nq: u64) -> Node {
    if c.phase is Stopping {
        Node { question: Some(Question { id: nq, kind: QKind::CleanupFailed, cont: Cont::Nowhere,
            cost: 0, clarified: false }), ..c }
    } else {
        let m = if matches!(kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase) {
            Node { run: None, job: None, pool: false, effect: match c.effect {
                Some(e) => Some(InFlight { cancelling: e.cancelling
                    || matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks
                        | EffectKind::StartRebase), ..e }),
                None => None }, ..c }
        } else { c };
        let cont = match m.phase {
            Phase::Ingesting => Cont::Ingesting,
            Phase::Auditing => Cont::Auditing,
            Phase::Planning { must_split } => Cont::Planning { must_split },
            Phase::Making => Cont::Ready(Resume::Making),
            Phase::Checking => Cont::Ready(Resume::Checking),
            Phase::Reviewing => Cont::Ready(Resume::Reviewing),
            Phase::Publishing => Cont::Ready(Resume::Publishing),
            Phase::Rebasing => Cont::Ready(Resume::Rebasing),
            _ => Cont::Unplanned,
        };
        retirement::phased(Node { holds: false, question: Some(Question { id: nq,
            kind: QKind::EffectFailed, cont, cost: 0, clarified: false }), ..m }, Phase::AwaitingRob)
    }
}

/// K5 G4: on_ack's guards; a stale, mismatched or not-yet-due ack changes nothing.
pub(super) open spec fn ack_stutters(b: Node, seq: u64, ack: Ack) -> bool {
    match b.effect {
        None => true,
        Some(e) => e.seq != seq
            || (ack == Ack::DeliveryAccepted && e.kind != EffectKind::Enqueue)
            || (!e.cancelling && e.kind == EffectKind::Enqueue && (ack == Ack::Confirmed
                || (ack == Ack::DeliveryAccepted && (b.phase != Phase::Publishing
                    || current_item(b) != Some(Item::Do(EffectKind::Enqueue)))))),
    }
}

/// K5 G4: exact facts of an admitted ack at node i, b before and n after.
#[verifier::opaque]
pub(super) open spec fn ack_live_post(b: Node, n: Node, e: InFlight, ack: Ack, nq: u64, nqa: u64,
    i: int, ob: Outbox, oa: Outbox) -> bool {
    let c = Node { effect: None, ..b };
    (nqa as int == if !e.cancelling && (ack == Ack::Permanent
        || (ack == Ack::BodyPolicyRefused && e.kind != EffectKind::OpenPr)) { nq as int + 1 } else { nq as int })
    && (e.cancelling ==> n == c && oa == ob)
    && (!e.cancelling ==> {
        n.proposed == b.proposed && (b.adopted ==> n.adopted) && n.credit == b.credit
        && n.next_seq >= b.next_seq && n.attempt == b.attempt && n.replanned == b.replanned
        && (n.phase is Planning ==> b.phase is Planning)
        && (n.question == b.question || match n.question {
            Some(q) => q.kind == QKind::EffectFailed || q.kind == QKind::CleanupFailed,
            None => true,
        })
        && ob.len <= oa.len
        && (forall|k: int| 0 <= k < ob.len ==> #[trigger] oa.items@[k] == ob.items@[k])
        && (forall|k: int| ob.len <= k < oa.len ==> match #[trigger] oa.items@[k] {
            Some(Out::Notice { node, .. }) => node == NodeId(i as u64),
            Some(Out::Effect { node, seq, kind, head }) => node == NodeId(i as u64)
                && seq == b.next_seq && seq < n.next_seq && kind == e.kind
                && head == effect_head(e.kind, b.head)
                && n.approved == b.approved && n.checked == b.checked,
            None => false,
        })
        && (ack == Ack::Confirmed ==> n == agenda_spec(resources_spec(c, e.kind), e.kind) && oa == ob)
        && ((ack == Ack::Permanent || (ack == Ack::BodyPolicyRefused && e.kind != EffectKind::OpenPr))
            ==> n == failed_spec(c, e.kind, nq) && oa.len == ob.len + 2
                && oa.items@ == ob.items@.update(ob.len as int, Some(Out::Notice { node: NodeId(i as u64),
                    notice: Notice::Incident { needs_rob: true } })).update(ob.len as int + 1,
                    Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Ask { question: nq } })))
        && (ack == Ack::BodyPolicyRefused && e.kind == EffectKind::OpenPr
            ==> n == retirement::stopped(Node { review_stop: Some(ReviewStop::PublicationBody), ..c },
                Outcome::Stopped) && oa == ob)
        && (!(ack == Ack::BodyPolicyRefused && e.kind == EffectKind::OpenPr) ==> n.review_stop == b.review_stop)
    })
}

pub(super) open spec fn ack_case_common(b: Node, n: Node, e: InFlight, i: int, ob: Outbox, oa: Outbox) -> bool {
    n.proposed == b.proposed && (b.adopted ==> n.adopted) && n.credit == b.credit
        && n.next_seq >= b.next_seq && n.attempt == b.attempt && n.replanned == b.replanned
        && (n.phase is Planning ==> b.phase is Planning)
        && (n.question == b.question || match n.question {
            Some(q) => q.kind == QKind::EffectFailed || q.kind == QKind::CleanupFailed,
            None => true,
        })
        && ob.len <= oa.len
        && (forall|k: int| 0 <= k < ob.len ==> #[trigger] oa.items@[k] == ob.items@[k])
        && (forall|k: int| ob.len <= k < oa.len ==> match #[trigger] oa.items@[k] {
            Some(Out::Notice { node, .. }) => node == NodeId(i as u64),
            Some(Out::Effect { node, seq, kind, head }) => node == NodeId(i as u64)
                && seq == b.next_seq && seq < n.next_seq && kind == e.kind
                && head == effect_head(e.kind, b.head)
                && n.approved == b.approved && n.checked == b.checked,
            None => false,
        })
}

proof fn ack_cancel_post(b: Node, n: Node, e: InFlight, ack: Ack, nq: u64, nqa: u64, i: int, ob: Outbox, oa: Outbox)
    requires e.cancelling, n == (Node { effect: None, ..b }), nqa == nq, oa == ob,
    ensures ack_live_post(b, n, e, ack, nq, nqa, i, ob, oa),
{
    reveal(ack_live_post);
}

proof fn ack_confirmed_post(b: Node, n: Node, e: InFlight, ack: Ack, nq: u64, nqa: u64, i: int, ob: Outbox, oa: Outbox)
    requires !e.cancelling, ack_case_common(b, n, e, i, ob, oa),
        ack == Ack::Confirmed, nqa == nq, oa == ob,
        n == agenda_spec(resources_spec(Node { effect: None, ..b }, e.kind), e.kind),
        n.review_stop == b.review_stop,
    ensures ack_live_post(b, n, e, ack, nq, nqa, i, ob, oa),
{
    reveal(ack_live_post);
}

proof fn ack_delivery_post(b: Node, n: Node, e: InFlight, ack: Ack, nq: u64, nqa: u64, i: int, ob: Outbox, oa: Outbox)
    requires !e.cancelling, ack_case_common(b, n, e, i, ob, oa),
        ack == Ack::DeliveryAccepted, nqa == nq, oa == ob,
        n.review_stop == b.review_stop,
    ensures ack_live_post(b, n, e, ack, nq, nqa, i, ob, oa),
{
    reveal(ack_live_post);
}

proof fn ack_retry_post(b: Node, n: Node, e: InFlight, ack: Ack, nq: u64, nqa: u64, i: int, ob: Outbox, oa: Outbox)
    requires !e.cancelling, ack_case_common(b, n, e, i, ob, oa),
        ack == Ack::Retryable, nqa == nq, n.review_stop == b.review_stop,
    ensures ack_live_post(b, n, e, ack, nq, nqa, i, ob, oa),
{
    reveal(ack_live_post);
}

proof fn ack_permanent_post(b: Node, n: Node, e: InFlight, ack: Ack, nq: u64, nqa: u64, i: int, ob: Outbox, oa: Outbox)
    requires !e.cancelling, ack_case_common(b, n, e, i, ob, oa),
        ack == Ack::Permanent || (ack == Ack::BodyPolicyRefused && e.kind != EffectKind::OpenPr),
        nqa as int == nq as int + 1, n.review_stop == b.review_stop,
        n == failed_spec(Node { effect: None, ..b }, e.kind, nq),
        oa.len == ob.len + 2,
        oa.items@ == ob.items@.update(ob.len as int, Some(Out::Notice { node: NodeId(i as u64),
            notice: Notice::Incident { needs_rob: true } })).update(ob.len as int + 1,
            Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Ask { question: nq } })),
    ensures ack_live_post(b, n, e, ack, nq, nqa, i, ob, oa),
{
    reveal(ack_live_post);
}

proof fn ack_review_post(b: Node, n: Node, e: InFlight, ack: Ack, nq: u64, nqa: u64, i: int, ob: Outbox, oa: Outbox)
    requires !e.cancelling, ack_case_common(b, n, e, i, ob, oa),
        ack == Ack::BodyPolicyRefused, e.kind == EffectKind::OpenPr, nqa == nq, oa == ob,
        n == retirement::stopped(Node { review_stop: Some(ReviewStop::PublicationBody), effect: None, ..b }, Outcome::Stopped),
    ensures ack_live_post(b, n, e, ack, nq, nqa, i, ob, oa),
{
    reveal(ack_live_post);
}

/// K5 G4: on_ack's exported post at node i for any outbox prefix with capacity.
#[verifier::opaque]
pub(super) open spec fn on_ack_post(before: World, after: World, i: int, seq: u64, ack: Ack,
    ob: Outbox, oa: Outbox) -> bool {
    let b = before.nodes@[i];
    let n = after.nodes@[i];
    if ack_stutters(b, seq, ack) {
        n == b && oa == ob && after.next_question == before.next_question
    } else {
        match b.effect {
            Some(e) => ack_live_post(b, n, e, ack, before.next_question, after.next_question, i, ob, oa),
            None => false,
        }
    }
}
}
