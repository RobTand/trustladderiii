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
] {
    let (node, next_question, output) = ack_value(w.nodes[i], i, e, ack, w.next_question, *out);
    kernel_proof! { ack_result_properties(*w, i as int, e, ack, *out, node, next_question, output); }
    w.nodes[i] = node;
    w.next_question = next_question;
    *out = output;
}
}

kernel_fn! {
/// PRD #878 / TD acceptance: release ownership and preserve approved delivery evidence.
#[allow(dead_code)] // Debt R2 lint: retained verified helper
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
#[allow(dead_code)] // Debt R2 lint: retained verified helper
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
#[allow(dead_code)] // Debt R2 lint: retained verified helper
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
#[allow(dead_code)] // Debt R2 lint: retained verified helper
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
#[allow(dead_code)] // Debt R2 lint: retained verified helper
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
#[allow(clippy::manual_map)] // Debt R2 lint: preserve the qualified executable shape
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
#[allow(clippy::collapsible_if)] // Debt R2 lint: preserve the qualified executable shape
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
#[allow(dead_code)] // Debt R2 lint: retained verified helper
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
#[allow(dead_code)] // Debt R2 lint: retained verified helper
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
#[allow(dead_code)] // Debt R2 lint: retained verified helper
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
    let (node, next_question, output) = ack_permanent_value(w.nodes[i], i, kind, w.next_question, *out);
    kernel_proof! { ack_permanent_rows(w.nodes@[i as int], i as int, kind, w.next_question, *out); }
    w.nodes[i] = node;
    w.next_question = next_question;
    *out = output;
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

kernel_fn! {
#[allow(clippy::manual_map)] // Debt R2 lint: preserve the qualified executable shape
fn ack_phase_value(n: Node, phase: Phase) -> (result: Node) [
    requires n.generation < u64::MAX,
    ensures result == retirement::phased(n, phase),
] {
    Node { generation: n.generation + 1, phase, pos: 0, ticket: None,
        effect: match n.effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None }, ..n }
}
}

kernel_fn! {
#[allow(clippy::manual_map)] // Debt R2 lint: preserve the qualified executable shape
fn ack_abandon_value(n: Node) -> (result: Node) [
    ensures result == ack_abandon_spec(n),
] {
    Node { run: None, job: None, pool: false,
        effect: match n.effect {
            Some(e) => Some(InFlight { cancelling: e.cancelling || e.kind.starts_operation(), ..e }),
            None => None,
        }, ..n }
}
}

kernel_fn! {
fn ack_append_value(out: Outbox, item: Out) -> (result: Outbox) [
    requires out.len < OUTBOX,
    ensures result.len == out.len + 1,
        result.items@ == out.items@.update(out.len as int, Some(item)),
] {
    let mut output = out;
    output.push(Out::Notice { node: NodeId(0), notice: Notice::Incident { needs_rob: false } });
    output
}
}

kernel_fn! {
fn ack_resources_value(n: Node, kind: EffectKind) -> (result: Node) [
    ensures result == resources_spec(n, kind),
] {
    Node { fx_failures: 0, fx_incidents: 0,
        run: if kind == EffectKind::StopRun { None } else { n.run },
        pool: if kind == EffectKind::StopRun { false } else { n.pool },
        job: if kind == EffectKind::WithdrawJob { None } else { n.job },
        pr_open: if kind == EffectKind::ClosePr { false } else { n.pr_open },
        worktree: if kind == EffectKind::RemoveWorktree { false } else { n.worktree }, ..n }
}
}

kernel_fn! {
fn ack_emit_value(n: Node, i: usize, kind: EffectKind, out: Outbox) -> (result: (Node, u64, Outbox)) [
    requires n.next_seq < u64::MAX, out.len < OUTBOX,
    ensures ack_view(result) == ack_emit_spec(n, i as int, kind, out),
] proof [reveal(ack_emit_spec);] {
    let seq = n.next_seq;
    let m = Node { next_seq: seq + 1,
        effect: Some(InFlight { seq, kind, cancelling: false }),
        worktree: n.worktree || kind == EffectKind::PrepareWorktree,
        pr_open: n.pr_open || kind == EffectKind::OpenPr,
        had_pr: n.had_pr || kind == EffectKind::OpenPr, ..n };
    let head = match kind {
        EffectKind::Push | EffectKind::PostApproval | EffectKind::OpenPr | EffectKind::Enqueue => m.head,
        _ => None,
    };
    let output = ack_append_value(out, Out::Effect { node: NodeId(i as u64), seq, kind, head });
    (m, 0, output)
}
}

kernel_fn! {
fn ack_stopped_value(n: Node) -> (result: Node) [
    requires n.generation < u64::MAX,
    ensures result == retirement::stopped(n, Outcome::Stopped),
] {
    kernel_proof! { allocation_stopping_lemmas::stopped_cases(n, Outcome::Stopped); }
    match n.phase {
        Phase::Closed(_) | Phase::Stopping(_) => n,
        _ => ack_phase_value(Node { question: None, waiting_incident: false,
            incident_needs_rob: false, ..n }, Phase::Stopping(Outcome::Stopped)),
    }
}
}

kernel_fn! {
fn ack_cancel_value(n: Node, nq: u64, out: Outbox) -> (result: (Node, u64, Outbox)) [
    ensures ack_view(result) == ack_cancel_spec(n, nq, out), result.2 == out,
] proof [reveal(ack_cancel_spec);] { (n, nq, out) }
}

kernel_fn! {
fn ack_confirmed_value(n: Node, kind: EffectKind, nq: u64, out: Outbox) -> (result: (Node, u64, Outbox)) [
    requires n.generation < u64::MAX,
    ensures ack_view(result) == ack_confirmed_spec(n, kind, nq, out), result.2 == out,
] proof [reveal(ack_confirmed_spec);] {
    let m = ack_resources_value(n, kind);
    let m = confirmed_agenda_value(m, kind);
    (m, nq, out)
}
}

kernel_fn! {
fn ack_delivery_value(n: Node, nq: u64, out: Outbox) -> (result: (Node, u64, Outbox)) [
    requires n.generation < u64::MAX,
    ensures ack_view(result) == ack_delivery_spec(n, nq, out), result.2 == out,
] proof [reveal(ack_delivery_spec);] {
    let m = Node { fx_failures: 0, fx_incidents: 0, holds: false, run: None,
        job: None, pool: false, question: None, ..n };
    let m = ack_phase_value(m, Phase::AwaitingDelivery);
    (m, nq, out)
}
}

kernel_fn! {
fn ack_retry_emit_value(n: Node, i: usize, kind: EffectKind, nq: u64, out: Outbox) -> (result: (Node, u64, Outbox)) [
    requires n.next_seq < u64::MAX, out.len < OUTBOX,
    ensures ack_view(result) == ack_retry_emit_spec(n, i as int, kind, nq, out),
] proof [reveal(ack_retry_emit_spec); reveal(ack_emit_spec);] {
    let (m, _, output) = ack_emit_value(n, i, kind, out);
    (m, nq, output)
}
}

kernel_fn! {
fn ack_incident_count(c: u8) -> (result: u8) [
    requires c <= 2,
    ensures result == ack_incident_count_spec(c),
] {
    (c + 1).min(2)
}
}

kernel_fn! {
fn ack_retry_restart_value(n: Node, i: usize, nq: u64, out: Outbox) -> (result: (Node, u64, Outbox)) [
    requires n.generation as int + 2 <= u64::MAX as int,
        n.fx_incidents <= 2, out.len < OUTBOX,
    ensures ack_view(result) == ack_retry_restart_spec(n, i as int, nq, out),
] {
    let count = ack_incident_count(n.fx_incidents);
    let m = Node { fx_failures: 0, fx_incidents: count, ..n };
    let needs_rob = m.fx_incidents >= 2;
    let m = ack_abandon_value(m);
    let m = Node { pos: operation_start(m.phase), generation: m.generation + 1, ..m };
    let (m, _) = incident_value(m, i, needs_rob);
    let output = ack_append_value(out, Out::Notice { node: NodeId(i as u64), notice: Notice::Incident { needs_rob } });
    (m, nq, output)
}
}

kernel_fn! {
fn ack_retry_plain_incident_value(n: Node, i: usize, nq: u64, out: Outbox) -> (result: (Node, u64, Outbox)) [
    requires n.generation as int + 2 <= u64::MAX as int,
        n.fx_incidents <= 2, out.len < OUTBOX,
    ensures ack_view(result) == ack_retry_plain_incident_spec(n, i as int, nq, out),
] {
    let count = ack_incident_count(n.fx_incidents);
    let m = Node { fx_failures: 0, fx_incidents: count, ..n };
    let needs_rob = m.fx_incidents >= 2;
    let (m, _) = incident_value(m, i, needs_rob);
    let output = ack_append_value(out, Out::Notice { node: NodeId(i as u64), notice: Notice::Incident { needs_rob } });
    (m, nq, output)
}
}

kernel_fn! {
fn ack_retry_incident_value(n: Node, i: usize, kind: EffectKind, nq: u64, out: Outbox) -> (result: (Node, u64, Outbox)) [
    requires n.generation as int + 2 <= u64::MAX as int,
        n.fx_incidents <= 2, out.len < OUTBOX,
    ensures ack_view(result) == ack_retry_incident_spec(n, i as int, kind, nq, out),
] {
    let result = if kind.starts_operation() {
        ack_retry_restart_value(n, i, nq, out)
    } else { ack_retry_plain_incident_value(n, i, nq, out) };
    kernel_proof! { ack_retry_incident_bridge(n, i as int, kind, nq, out); }
    result
}
}

kernel_fn! {
#[allow(clippy::let_and_return)] // Debt R2 lint: preserve the qualified executable shape
fn ack_retry_value(n: Node, i: usize, kind: EffectKind, nq: u64, out: Outbox) -> (result: (Node, u64, Outbox)) [
    requires n.generation as int + 2 <= u64::MAX as int, n.next_seq < u64::MAX,
        n.fx_failures <= 1, n.fx_incidents <= 2, out.len < OUTBOX,
    ensures ack_view(result) == ack_retry_spec(n, i as int, kind, nq, out),
] proof [reveal(ack_retry_spec);] {
    let m = Node { fx_failures: n.fx_failures + 1, ..n };
    let result = if m.fx_failures < 2 {
        ack_retry_emit_value(m, i, kind, nq, out)
    } else { ack_retry_incident_value(m, i, kind, nq, out) };
    result
}
}

kernel_fn! {
fn ack_permanent_value(n: Node, i: usize, kind: EffectKind, nq: u64, out: Outbox) -> (result: (Node, u64, Outbox)) [
    requires n.generation < u64::MAX, nq < u64::MAX, out.len + 2 <= OUTBOX,
    ensures ack_view(result) == ack_permanent_spec(n, i as int, kind, nq, out),
] proof [reveal(ack_permanent_spec);] {
    let output = ack_append_value(out, Out::Notice { node: NodeId(i as u64), notice: Notice::Incident { needs_rob: true } });
    if n.phase.is_stopping() {
        let id = nq;
        let next_question = nq + 1;
        let m = Node { question: Some(Question { id, kind: QKind::CleanupFailed,
            cont: Cont::Nowhere, cost: 0, clarified: false }), ..n };
        let output = ack_append_value(output, Out::Notice { node: NodeId(i as u64), notice: Notice::Ask { question: id } });
        return (m, next_question, output);
    }
    let m = if kind.starts_operation() { ack_abandon_value(n) } else { n };
    let cont = match m.phase {
        Phase::Ingesting => Cont::Ingesting,
        Phase::Auditing => Cont::Auditing,
        Phase::Planning { must_split } => Cont::Planning { must_split },
        phase => match phase.resume() { Some(r) => Cont::Ready(r), None => Cont::Unplanned },
    };
    let id = nq;
    let next_question = nq + 1;
    let m = Node { holds: false, question: Some(Question { id, kind: QKind::EffectFailed,
        cont, cost: 0, clarified: false }), ..m };
    let m = ack_phase_value(m, Phase::AwaitingRob);
    let output = ack_append_value(output, Out::Notice { node: NodeId(i as u64), notice: Notice::Ask { question: id } });
    (m, next_question, output)
}
}

kernel_fn! {
fn ack_body_policy_value(n: Node, i: usize, kind: EffectKind, nq: u64, out: Outbox) -> (result: (Node, u64, Outbox)) [
    requires n.generation < u64::MAX, nq < u64::MAX, out.len + 2 <= OUTBOX,
    ensures ack_view(result) == ack_body_policy_spec(n, i as int, kind, nq, out),
        kind == EffectKind::OpenPr ==> result.2 == out,
] proof [reveal(ack_body_policy_spec);] {
    if kind == EffectKind::OpenPr {
        let m = ack_stopped_value(Node { review_stop: Some(ReviewStop::PublicationBody), ..n });
        (m, nq, out)
    } else { ack_permanent_value(n, i, kind, nq, out) }
}
}

kernel_fn! {
fn ack_value(n: Node, i: usize, e: InFlight, ack: Ack, nq: u64, out: Outbox) -> (result: (Node, u64, Outbox)) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX, out.len + 2 <= OUTBOX,
    ensures ack_view(result) == ack_value_spec(n, i as int, e, ack, nq, out),
        ack_quiet(e, ack) ==> result.2 == out,
] {
    let mut n = n;
    n.effect = None;
    if e.cancelling { return ack_cancel_value(n, nq, out); }
    match ack {
        Ack::Confirmed => ack_confirmed_value(n, e.kind, nq, out),
        Ack::DeliveryAccepted => ack_delivery_value(n, nq, out),
        Ack::Retryable => ack_retry_value(n, i, e.kind, nq, out),
        Ack::Permanent => ack_permanent_value(n, i, e.kind, nq, out),
        Ack::BodyPolicyRefused => ack_body_policy_value(n, i, e.kind, nq, out),
    }
}
}

#[cfg(verus_keep_ghost)]
verus! {

pub(super) open spec fn ack_view(r: (Node, u64, Outbox)) -> (Node, u64, Seq<Option<Out>>, int) {
    (r.0, r.1, r.2.items@, r.2.len as int)
}
pub(super) open spec fn ack_abandon_spec(n: Node) -> Node {
    Node { run: None, job: None, pool: false,
        effect: match n.effect { Some(e) => Some(InFlight { cancelling: e.cancelling
            || matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }), None => None }, ..n }
}
#[verifier::opaque]
pub(super) open spec fn ack_cancel_spec(n: Node, nq: u64, out: Outbox) -> (Node, u64, Seq<Option<Out>>, int) {
    (n, nq, out.items@, out.len as int)
}
#[verifier::opaque]
pub(super) open spec fn ack_confirmed_spec(n: Node, kind: EffectKind, nq: u64, out: Outbox) -> (Node, u64, Seq<Option<Out>>, int) {
    (agenda_spec(resources_spec(n, kind), kind), nq, out.items@, out.len as int)
}
#[verifier::opaque]
pub(super) open spec fn ack_delivery_spec(n: Node, nq: u64, out: Outbox) -> (Node, u64, Seq<Option<Out>>, int) {
    (retirement::phased(Node { fx_failures: 0, fx_incidents: 0, holds: false,
        run: None, job: None, pool: false, question: None, ..n }, Phase::AwaitingDelivery), nq, out.items@, out.len as int)
}
#[verifier::opaque]
pub(super) open spec fn ack_emit_spec(n: Node, i: int, kind: EffectKind, out: Outbox) -> (Node, u64, Seq<Option<Out>>, int) {
    (Node { next_seq: (n.next_seq + 1) as u64, effect: Some(InFlight { seq: n.next_seq, kind, cancelling: false }),
        worktree: n.worktree || kind == EffectKind::PrepareWorktree,
        pr_open: n.pr_open || kind == EffectKind::OpenPr, had_pr: n.had_pr || kind == EffectKind::OpenPr, ..n }, 0u64,
        out.items@.update(out.len as int, Some(Out::Effect { node: NodeId(i as u64), seq: n.next_seq,
            kind, head: effect_head(kind, n.head) })), out.len as int + 1)
}
pub(super) open spec fn ack_retry_emit_spec(n: Node, i: int, kind: EffectKind, nq: u64, out: Outbox) -> (Node, u64, Seq<Option<Out>>, int) {
    let r = ack_emit_spec(n, i, kind, out);
    (r.0, nq, r.2, r.3)
}
pub(super) open spec fn ack_incident_count_spec(c: u8) -> u8 {
    if c < 2 { (c + 1) as u8 } else { 2u8 }
}
pub(super) open spec fn ack_retry_restart_spec(n: Node, i: int, nq: u64, out: Outbox) -> (Node, u64, Seq<Option<Out>>, int) {
    let m = Node { fx_failures: 0, fx_incidents: ack_incident_count_spec(n.fx_incidents), ..n };
    let m = ack_abandon_spec(m);
    let m = Node { pos: if m.phase == Phase::Making { 1u8 } else { 0u8 }, generation: (m.generation + 1) as u64, ..m };
    (retirement::incident(m, m.fx_incidents >= 2), nq,
        out.items@.update(out.len as int, Some(Out::Notice { node: NodeId(i as u64),
            notice: Notice::Incident { needs_rob: m.fx_incidents >= 2 } })), out.len as int + 1)
}
pub(super) open spec fn ack_retry_plain_incident_spec(n: Node, i: int, nq: u64, out: Outbox) -> (Node, u64, Seq<Option<Out>>, int) {
    let m = Node { fx_failures: 0, fx_incidents: ack_incident_count_spec(n.fx_incidents), ..n };
    (retirement::incident(m, m.fx_incidents >= 2), nq,
        out.items@.update(out.len as int, Some(Out::Notice { node: NodeId(i as u64),
            notice: Notice::Incident { needs_rob: m.fx_incidents >= 2 } })), out.len as int + 1)
}
pub(super) open spec fn ack_retry_incident_arms(n: Node, i: int, kind: EffectKind, nq: u64, out: Outbox) -> (Node, u64, Seq<Option<Out>>, int) {
    if matches!(kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase) {
        ack_retry_restart_spec(n, i, nq, out)
    } else { ack_retry_plain_incident_spec(n, i, nq, out) }
}
pub(super) proof fn ack_retry_incident_bridge(n: Node, i: int, kind: EffectKind, nq: u64, out: Outbox)
    ensures ack_retry_incident_arms(n, i, kind, nq, out) == ack_retry_incident_spec(n, i, kind, nq, out),
{
    reveal(ack_retry_incident_spec);
}
#[verifier::opaque]
pub(super) open spec fn ack_retry_incident_spec(n: Node, i: int, kind: EffectKind, nq: u64, out: Outbox) -> (Node, u64, Seq<Option<Out>>, int) {
    let m = Node { fx_failures: 0, fx_incidents: if n.fx_incidents < 2 { (n.fx_incidents + 1) as u8 } else { 2u8 }, ..n };
    let m = if matches!(kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase) {
        let m = ack_abandon_spec(m);
        Node { pos: if m.phase == Phase::Making { 1u8 } else { 0u8 }, generation: (m.generation + 1) as u64, ..m }
    } else { m };
    (retirement::incident(m, m.fx_incidents >= 2), nq,
        out.items@.update(out.len as int, Some(Out::Notice { node: NodeId(i as u64),
            notice: Notice::Incident { needs_rob: m.fx_incidents >= 2 } })), out.len as int + 1)
}
#[verifier::opaque]
pub(super) open spec fn ack_retry_spec(n: Node, i: int, kind: EffectKind, nq: u64, out: Outbox) -> (Node, u64, Seq<Option<Out>>, int) {
    let m = Node { fx_failures: (n.fx_failures + 1) as u8, ..n };
    if m.fx_failures < 2 { ack_retry_emit_spec(m, i, kind, nq, out) }
    else { ack_retry_incident_spec(m, i, kind, nq, out) }
}

#[verifier::opaque]
pub(super) open spec fn ack_permanent_spec(n: Node, i: int, kind: EffectKind, nq: u64, out: Outbox) -> (Node, u64, Seq<Option<Out>>, int) {
    (failed_spec(n, kind, nq), (nq + 1) as u64,
        out.items@.update(out.len as int, Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Incident { needs_rob: true } }))
            .update(out.len as int + 1, Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Ask { question: nq } })), out.len as int + 2)
}
#[verifier::opaque]
pub(super) open spec fn ack_body_policy_spec(n: Node, i: int, kind: EffectKind, nq: u64, out: Outbox) -> (Node, u64, Seq<Option<Out>>, int) {
    if kind == EffectKind::OpenPr {
        (retirement::stopped(Node { review_stop: Some(ReviewStop::PublicationBody), ..n }, Outcome::Stopped), nq, out.items@, out.len as int)
    } else { ack_permanent_spec(n, i, kind, nq, out) }
}
pub(super) open spec fn ack_quiet(e: InFlight, ack: Ack) -> bool {
    e.cancelling || ack == Ack::Confirmed || ack == Ack::DeliveryAccepted
        || (ack == Ack::BodyPolicyRefused && e.kind == EffectKind::OpenPr)
}
pub(super) open spec fn ack_value_spec(b: Node, i: int, e: InFlight, ack: Ack, nq: u64, out: Outbox) -> (Node, u64, Seq<Option<Out>>, int) {
    let n = Node { effect: None, ..b };
    if e.cancelling { ack_cancel_spec(n, nq, out) } else { match ack {
        Ack::Confirmed => ack_confirmed_spec(n, e.kind, nq, out),
        Ack::DeliveryAccepted => ack_delivery_spec(n, nq, out),
        Ack::Retryable => ack_retry_spec(n, i, e.kind, nq, out),
        Ack::Permanent => ack_permanent_spec(n, i, e.kind, nq, out),
        Ack::BodyPolicyRefused => ack_body_policy_spec(n, i, e.kind, nq, out),
    } }
}

// Exact result bridge for the legacy permanent-failure caller.
proof fn ack_permanent_rows(n: Node, i: int, kind: EffectKind, nq: u64, out: Outbox)
    requires n.generation < u64::MAX, nq < u64::MAX,
    ensures ack_permanent_spec(n, i, kind, nq, out).0.repo == n.repo,
        ack_permanent_spec(n, i, kind, nq, out).0.footprint == n.footprint,
        (settle_shape(n) ==> settle_shape(ack_permanent_spec(n, i, kind, nq, out).0)),
        ack_permanent_spec(n, i, kind, nq, out).0.generation as int <= n.generation as int + 1,
        ack_permanent_spec(n, i, kind, nq, out).0.next_seq == n.next_seq,
        ack_permanent_spec(n, i, kind, nq, out).0.credit == n.credit,
        ack_permanent_spec(n, i, kind, nq, out).0.last_cmd == n.last_cmd,
        ack_permanent_spec(n, i, kind, nq, out).0 == failed_spec(n, kind, nq),
        ack_permanent_spec(n, i, kind, nq, out).1 as int == nq as int + 1,
        ack_permanent_spec(n, i, kind, nq, out).2 == out.items@.update(out.len as int,
            Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Incident { needs_rob: true } }))
            .update(out.len as int + 1,
                Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Ask { question: nq } })),
        ack_permanent_spec(n, i, kind, nq, out).3 == out.len as int + 2,
{
    reveal(ack_permanent_spec);
}

proof fn ack_result_properties(w: World, i: int, e: InFlight, ack: Ack, ob: Outbox,
    n: Node, nqa: u64, oa: Outbox)
    requires 0 <= i < w.nodes@.len(), ob.len + 2 <= OUTBOX,
        node_headroom(w.nodes@[i]), arithmetic_consistent(w.nodes@[i]), w.next_question < u64::MAX,
        w.nodes@[i].effect == Some(e),
        ack == Ack::DeliveryAccepted ==> e.kind == EffectKind::Enqueue,
        !e.cancelling && e.kind == EffectKind::Enqueue ==> ack != Ack::Confirmed
            && (ack == Ack::DeliveryAccepted ==> w.nodes@[i].phase == Phase::Publishing
                && current_item(w.nodes@[i]) == Some(Item::Do(EffectKind::Enqueue))),
        ack_view((n, nqa, oa)) == ack_value_spec(w.nodes@[i], i, e, ack, w.next_question, ob),
        ack_quiet(e, ack) ==> oa == ob,
    ensures (node_checked(w, w.nodes@[i])
            && (!w.nodes@[i].paused || w.nodes@[i].phase is Stopping)
            && (if e.kind == EffectKind::Enqueue { ack == Ack::DeliveryAccepted } else { ack == Ack::Confirmed })
            ==> distance(n) < distance(w.nodes@[i])),
        n.repo == w.nodes@[i].repo, n.footprint == w.nodes@[i].footprint,
        (settle_shape(w.nodes@[i]) && stopping_effect_fits(w.nodes@[i]) ==> settle_shape(n)),
        n.generation as int <= w.nodes@[i].generation as int + 2,
        n.next_seq as int <= w.nodes@[i].next_seq as int + 1,
        n.credit == w.nodes@[i].credit, nqa as int <= w.next_question as int + 1,
        oa.len <= ob.len + 2, n.last_cmd == w.nodes@[i].last_cmd,
        ack_live_post(w.nodes@[i], n, e, ack, w.next_question, nqa, i, ob, oa),
{
    let b = w.nodes@[i];
    let c = Node { effect: None, ..b };
    reveal(distance);
    if e.cancelling {
        reveal(ack_cancel_spec);
        ack_cancel_post(b, n, e, ack, w.next_question, nqa, i, ob, oa);
    } else {
        match ack {
            Ack::Confirmed => {
                reveal(ack_confirmed_spec);
                let r = resources_spec(c, e.kind);
                agenda_result_properties(r, n, e.kind);
                cleanup_lemmas::resource_distance(c, r);
                ack_confirmed_post(b, n, e, ack, w.next_question, nqa, i, ob, oa);
            },
            Ack::DeliveryAccepted => { reveal(ack_delivery_spec); ack_delivery_post(b, n, e, ack, w.next_question, nqa, i, ob, oa); },
            Ack::Retryable => {
                reveal(ack_retry_spec); reveal(ack_retry_emit_spec); reveal(ack_emit_spec);
                reveal(ack_retry_incident_spec); reveal(retirement::incident);
                ack_retry_post(b, n, e, ack, w.next_question, nqa, i, ob, oa);
            },
            Ack::Permanent => { reveal(ack_permanent_spec); ack_permanent_post(b, n, e, ack, w.next_question, nqa, i, ob, oa); },
            Ack::BodyPolicyRefused => {
                reveal(ack_body_policy_spec);
                if e.kind == EffectKind::OpenPr {
                    allocation_stopping_lemmas::stopped_cases(Node { review_stop: Some(ReviewStop::PublicationBody), ..c }, Outcome::Stopped);
                    allocation_stopping_lemmas::stopping_frame(Node { review_stop: Some(ReviewStop::PublicationBody), ..c }, n, Outcome::Stopped);
                    ack_review_post(b, n, e, ack, w.next_question, nqa, i, ob, oa);
                } else { reveal(ack_permanent_spec); ack_permanent_post(b, n, e, ack, w.next_question, nqa, i, ob, oa); }
            },
        }
    }
}

}
