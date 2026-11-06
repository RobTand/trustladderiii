//! #806: exact local frames for the real settle dispatcher and its outputs.
use super::*;

verus! {
/// State-table paused holding rows: release the slot and retain the resume point.
pub(super) open spec fn paused(n: Node) -> Node {
    if holding(n.phase) {
        retirement::phased(Node { holds: false, ..n }, Phase::Ready(match n.phase {
            Phase::Making => Resume::Making,
            Phase::Checking => Resume::Checking,
            Phase::Reviewing => Resume::Reviewing,
            Phase::Publishing | Phase::Queued => Resume::Publishing,
            _ => Resume::Rebasing,
        }))
    } else { Node { ticket: None, ..n } }
}

/// State-table effect machine: the exact allocation update, including PR flags.
pub(super) open spec fn emitted(n: Node, kind: EffectKind) -> Node {
    Node {
        next_seq: (n.next_seq + 1) as u64,
        effect: Some(InFlight { seq: n.next_seq, kind, cancelling: false }),
        worktree: n.worktree || kind == EffectKind::PrepareWorktree,
        pr_open: n.pr_open || kind == EffectKind::OpenPr,
        had_pr: n.had_pr || kind == EffectKind::OpenPr,
        ..n
    }
}

/// State-table closing function: credit Merged once, retaining every other field.
pub(super) open spec fn closed(n: Node, outcome: Outcome) -> Node {
    Node { phase: Phase::Closed(outcome), pos: 0, ticket: None,
        credit: Credit {
            earned: (n.credit.earned
                + if outcome == Outcome::Merged && !n.credit.merge_credited { 1int } else { 0int }) as u64,
            merge_credited: n.credit.merge_credited || outcome == Outcome::Merged,
            ..n.credit
        }, ..n }
}

/// State-table Stopping: a missing resource consumes its teardown agenda entry.
pub(super) open spec fn gone(n: Node, kind: EffectKind) -> bool {
    match kind {
        EffectKind::StopRun => n.run.is_none(),
        EffectKind::WithdrawJob => n.job.is_none(),
        EffectKind::ClosePr => !n.pr_open,
        EffectKind::RemoveWorktree => !n.worktree,
        _ => false,
    }
}

/// #806 / Q30 / Q39: exact node poststate of the real local dispatcher.
#[verifier::opaque]
pub(super) open spec fn node_after(n: Node, ticket: u64, c: AdvanceContext) -> Node {
    if n.phase is Closed { n }
    else if n.waiting_incident { Node { ticket: None, ..n } }
    else if n.effect.is_some() { n }
    else { match current_item(n) {
        None => match n.phase {
            Phase::Publishing => retirement::phased(n, Phase::Queued),
            Phase::Stopping(outcome) => if n.question.is_none() { closed(n, outcome) } else { n },
            _ => n,
        },
        Some(Item::Do(kind)) => if n.paused { paused(n) } else { emitted(n, kind) },
        Some(Item::Start(role)) => if n.paused { paused(n) }
            else if n.ticket.is_none() { Node { ticket: Some(ticket), ..n } }
            else if c.pool { emitted(Node { ticket: None, pool: true,
                run: Some(n.generation), pos: (n.pos + 1) as u8, ..n }, EffectKind::StartRun(role)) }
            else { n },
        Some(Item::StartChecks) => if n.paused { paused(n) }
            else { emitted(Node { job: Some(n.generation), pos: (n.pos + 1) as u8, ..n },
                EffectKind::SubmitChecks) },
        Some(Item::StartRebase) => if n.paused { paused(n) }
            else { emitted(Node { run: Some(n.generation), pos: (n.pos + 1) as u8, ..n },
                EffectKind::StartRebase) },
        Some(Item::Admit) => if n.paused || !c.admit { n }
            else { retirement::phased(Node { credit: Credit { withheld: false, ..n.credit },
                planning_started: true, ..n }, Phase::Planning { must_split: false }) },
        Some(Item::Slot) => match n.phase {
            Phase::Ready(r) => if n.paused { Node { ticket: None, ..n } }
                else if n.ticket.is_none() { Node { ticket: Some(ticket), ..n } }
                else if c.slot { retirement::phased(Node { holds: true, ..n }, resumed_phase(r)) }
                else { n },
            _ => n,
        },
        Some(Item::Release) => Node { holds: false, pos: (n.pos + 1) as u8, ..n },
        Some(Item::Teardown(kind)) => if n.question.is_some() { n }
            else if gone(n, kind) { Node { pool: n.pool && kind != EffectKind::StopRun,
                pos: (n.pos + 1) as u8, ..n } }
            else { emitted(n, kind) },
        Some(Item::Await | Item::Merge | Item::Answer) => n,
    } }
}

/// #806 / Q39: exactly the ticket-taking branches allocate a global ticket.
pub(super) open spec fn takes_ticket(n: Node) -> bool {
    !(n.phase is Closed) && !n.waiting_incident && n.effect.is_none()
        && !n.paused && n.ticket.is_none()
        && matches!(current_item(n), Some(Item::Start(_)) | Some(Item::Slot))
}

/// State-table effect rows: select precisely the effect emitted by this step.
#[verifier::opaque]
pub(super) open spec fn effect_kind(n: Node, c: AdvanceContext) -> Option<EffectKind> {
    if n.phase is Closed || n.waiting_incident || n.effect.is_some() { None }
    else { match current_item(n) {
        Some(Item::Do(kind)) => if n.paused { None } else { Some(kind) },
        Some(Item::Start(role)) => if !n.paused && n.ticket.is_some() && c.pool {
            Some(EffectKind::StartRun(role))
        } else { None },
        Some(Item::StartChecks) => if n.paused { None } else { Some(EffectKind::SubmitChecks) },
        Some(Item::StartRebase) => if n.paused { None } else { Some(EffectKind::StartRebase) },
        Some(Item::Teardown(kind)) => if n.question.is_none() && !gone(n, kind) { Some(kind) }
            else { None },
        _ => None,
    } }
}

/// State-table effects: publication effects carry the observed node head.
pub(super) open spec fn effect_output(n: Node, i: usize, kind: EffectKind) -> Out {
    Out::Effect { node: NodeId(i as u64), seq: n.next_seq, kind,
        head: if matches!(kind, EffectKind::Push | EffectKind::PostApproval
            | EffectKind::OpenPr | EffectKind::Enqueue) { n.head } else { None } }
}

/// State-table output buffer: append one exact entry and preserve every other slot.
pub(super) open spec fn appended(before: Outbox, after: Outbox, item: Out) -> bool {
    after.len == before.len + 1
        && after.items@ == before.items@.update(before.len as int, Some(item))
}

/// #806: exact contents, order, and frame of outputs from one real advance.
#[verifier::opaque]
pub(super) open spec fn outputs(n: Node, c: AdvanceContext, before: Outbox, after: Outbox) -> bool {
    match effect_kind(n, c) {
        Some(kind) => appended(before, after, effect_output(n, c.node, kind)),
        None => if !(n.phase is Closed) && !n.waiting_incident && n.effect.is_none()
            && current_item(n).is_none() && n.question.is_none() && n.phase is Stopping {
            let outcome = match n.phase { Phase::Stopping(outcome) => outcome, _ => Outcome::Stopped };
            let summary = Some(Out::Notice { node: NodeId(c.node as u64), notice: Notice::Summary(outcome) });
            if outcome == Outcome::Replaced {
                after.len == before.len + 2 && after.items@ == before.items@
                    .update(before.len as int, summary)
                    .update(before.len as int + 1,
                        Some(Out::Notice { node: NodeId(c.node as u64), notice: Notice::FileReplacement }))
            } else { after.len == before.len + 1
                && after.items@ == before.items@.update(before.len as int, summary) }
        } else { after == before },
    }
}

/// #806: compose the exact local node, global ticket, and output frames.
#[verifier::opaque]
pub(super) open spec fn exact(before: Node, after: Node, ticket_before: u64, ticket_after: u64,
    c: AdvanceContext, out_before: Outbox, out_after: Outbox) -> bool {
    after == node_after(before, ticket_before, c)
        && ticket_after as int == ticket_before as int + if takes_ticket(before) { 1int } else { 0int }
        && outputs(before, c, out_before, out_after)
}

/// #806 / Q45 / Q54: settle preserves adoption and the recorded review decision.
pub(super) proof fn node_frame(n: Node, ticket: u64, c: AdvanceContext)
    ensures node_after(n, ticket, c).proposed == n.proposed,
        node_after(n, ticket, c).adopted == n.adopted,
        node_after(n, ticket, c).review_stop == n.review_stop,
        node_after(n, ticket, c).repo == n.repo,
        node_after(n, ticket, c).footprint == n.footprint,
{
    reveal(node_after);
}
}
