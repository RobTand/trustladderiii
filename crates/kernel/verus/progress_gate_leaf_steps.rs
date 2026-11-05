//! Q48 / #806: exact facts about the unchanged teardown and preparation steps.
use super::*;

verus! {
/// State-table effect emission: the real emit_node contract, without its outbox.
pub(super) open spec fn emitted(n: Node, kind: EffectKind) -> Node {
    Node { next_seq: (n.next_seq + 1) as u64,
        effect: Some(InFlight { seq: n.next_seq, kind, cancelling: false }),
        worktree: n.worktree || kind == EffectKind::PrepareWorktree,
        pr_open: n.pr_open || kind == EffectKind::OpenPr,
        had_pr: n.had_pr || kind == EffectKind::OpenPr, ..n }
}

/// #806: a Queued blocker's cleanup has no agent run, check job or pool grant.
pub(super) open spec fn teardown_shape(n: Node) -> bool {
    n.phase == Phase::Stopping(Outcome::Stopped) && !n.paused && !n.waiting_incident
        && n.question.is_none() && n.run.is_none() && n.job.is_none() && !n.pool && n.pos <= 5
}

/// #806: the only emitted cleanup effects of the original Queued blocker.
pub(super) open spec fn confirm_input(n: Node, kind: EffectKind) -> bool {
    teardown_shape(n) && n.effect.is_none()
        && (kind == EffectKind::ClosePr || kind == EffectKind::RemoveWorktree)
        && current_item(n) == Some(Item::Teardown(kind))
}

/// State-table teardown: a successful confirmation cleans exactly its resource.
#[verifier::opaque]
pub(super) open spec fn cleanup_confirmed(n: Node, kind: EffectKind) -> Node {
    Node { pos: (n.pos + 1) as u8, fx_failures: 0, fx_incidents: 0,
        run: if kind == EffectKind::StopRun { None } else { n.run },
        pool: if kind == EffectKind::StopRun { false } else { n.pool },
        job: if kind == EffectKind::WithdrawJob { None } else { n.job },
        pr_open: n.pr_open && kind != EffectKind::ClosePr,
        worktree: n.worktree && kind != EffectKind::RemoveWorktree, ..n }
}

/// State-table teardown: a matching live effect belongs to the current cleanup.
pub(super) open spec fn cleanup_ack_input(n: Node, seq: u64) -> bool {
    teardown_shape(n) && match n.effect {
        Some(e) => e.seq == seq && !e.cancelling
            && (e.kind == EffectKind::ClosePr || e.kind == EffectKind::RemoveWorktree)
            && current_item(n) == Some(Item::Teardown(e.kind)),
        None => false,
    }
}

/// State-table acknowledgement row: the effect is cleared before confirmation.
#[verifier::opaque]
pub(super) open spec fn cleanup_ack(n: Node) -> Node {
    cleanup_confirmed(Node { effect: None, ..n }, n.effect.unwrap().kind)
}

/// State-table teardown: inspect whether this cleanup target is already absent.
pub(super) open spec fn gone(n: Node, kind: EffectKind) -> bool {
    match kind {
        EffectKind::StopRun => n.run.is_none(), EffectKind::WithdrawJob => n.job.is_none(),
        EffectKind::ClosePr => !n.pr_open, EffectKind::RemoveWorktree => !n.worktree,
        _ => false,
    }
}

/// State-table teardown: one actual advance either skips, emits, releases or closes.
pub(super) open spec fn cleanup_advance(n: Node) -> Node {
    match current_item(n) {
        Some(Item::Release) => Node { holds: false, pos: (n.pos + 1) as u8, ..n },
        Some(Item::Teardown(kind)) => {
            if gone(n, kind) { Node { pos: (n.pos + 1) as u8,
                pool: n.pool && kind != EffectKind::StopRun, ..n } }
            else { emitted(n, kind) }
        },
        _ => Node { phase: Phase::Closed(Outcome::Stopped), pos: 0, ticket: None, ..n },
    }
}

/// #806: source-connected details added to the existing advance_post, without
/// restricting any caller. Other states keep their existing contracts.
#[verifier::opaque]
pub(super) open spec fn advance_details(before: Node, after: Node, ticket_before: u64, ticket_after: u64) -> bool {
    (teardown_shape(before) && before.effect.is_none() ==> after == cleanup_advance(before) && ticket_after == ticket_before)
    && (before.phase == Phase::Making && before.pos == 0 && !before.paused
        && !before.waiting_incident && before.effect.is_none()
        ==> after == emitted(before, EffectKind::PrepareWorktree) && ticket_after == ticket_before)
}

/// Stop row / #806: opaque exact update keeps handler rank queries small.
#[verifier::opaque]
pub(super) open spec fn stopped_command(before: Node, after: Node, id: u64) -> bool {
    after == retirement::stopped(Node { last_cmd: id, ..before }, Outcome::Stopped)
}

/// Stop row / #806: fold the real enter_stopping result at the call site.
pub(super) proof fn stopped_command_result(before: Node, after: Node, id: u64)
    requires after == retirement::stopped(Node { last_cmd: id, ..before }, Outcome::Stopped),
    ensures stopped_command(before, after, id),
{
    reveal(stopped_command);
}

/// #806: unrelated states satisfy the conditional step detail vacuously.
pub(super) proof fn inactive_details(n: Node, after: Node, before_ticket: u64, after_ticket: u64)
    requires !(teardown_shape(n) && n.effect.is_none()),
        !(n.phase == Phase::Making && n.pos == 0 && !n.paused && !n.waiting_incident && n.effect.is_none()),
    ensures advance_details(n, after, before_ticket, after_ticket),
{
    reveal(advance_details);
}

/// #806: a real blocked advance leaves every rank and allocation reserve equal.
pub(super) proof fn stutter_rank(n: Node, ticket: u64)
    ensures advance_post(n, n, ticket, ticket, false),
{
    reveal(advance_post);
}

/// #806: skipped teardown consumes one agenda item, including its exact ranks.
pub(super) proof fn skipped_rank(n: Node, after: Node, kind: EffectKind, ticket: u64)
    requires !n.waiting_incident, n.effect.is_none(), n.question.is_none(),
        current_item(n) == Some(Item::Teardown(kind)), gone(n, kind),
        after == (Node { pos: (n.pos + 1) as u8, pool: n.pool && kind != EffectKind::StopRun, ..n }),
    ensures advance_post(n, after, ticket, ticket, true),
{
    reveal(advance_post); reveal(distance); rank_definitions(n); rank_definitions(after);
    assert(n.phase is Stopping);
}

/// #806: emitting a real cleanup effect lowers its distance and consumes capacity.
pub(super) proof fn emitted_rank(n: Node, after: Node, kind: EffectKind, ticket: u64)
    requires !n.waiting_incident, n.effect.is_none(), n.question.is_none(),
        current_item(n) == Some(Item::Teardown(kind)), n.next_seq < u64::MAX,
        after == emitted(n, kind),
    ensures advance_post(n, after, ticket, ticket, true),
{
    reveal(advance_post); reveal(distance); rank_definitions(n); rank_definitions(after);
    assert(n.phase is Stopping);
}

/// #806: fold the exact skipped cleanup result away from rank arithmetic.
pub(super) proof fn skipped_details(n: Node, after: Node, kind: EffectKind, ticket: u64)
    requires current_item(n) == Some(Item::Teardown(kind)), n.question.is_none(), gone(n, kind),
        after == (Node { pos: (n.pos + 1) as u8, pool: n.pool && kind != EffectKind::StopRun, ..n }),
    ensures advance_details(n, after, ticket, ticket),
{
    reveal(advance_details);
}

/// #806: fold the exact emitted cleanup result away from rank arithmetic.
pub(super) proof fn emitted_details(n: Node, after: Node, kind: EffectKind, ticket: u64)
    requires current_item(n) == Some(Item::Teardown(kind)), n.question.is_none(), !gone(n, kind),
        after == emitted(n, kind),
    ensures advance_details(n, after, ticket, ticket),
{
    reveal(advance_details);
}

/// #806: five cleanup agenda entries; each advance plus confirmation consumes
/// exactly one. The sixth advance closes without emitting another cleanup effect.
pub(super) proof fn cleanup_round(n: Node, advanced: Node, after: Node)
    requires n.phase == Phase::Stopping(Outcome::Stopped), n.pos < 5,
        n.effect.is_none(), n.question.is_none(), !n.waiting_incident,
        n.run.is_none(), n.job.is_none(), !n.pool,
        advanced == cleanup_advance(n),
        after == if advanced.effect.is_some() { cleanup_ack(advanced) } else { advanced },
    ensures after.phase == n.phase, after.pos == n.pos + 1,
        after.effect.is_none(), after.question.is_none(), !after.waiting_incident,
        after.repo == n.repo, after.footprint == n.footprint,
        n.pos == 3 ==> !after.holds,
        n.pos != 3 ==> after.holds == n.holds,
{
    reveal(cleanup_ack); reveal(cleanup_confirmed);
}
}
