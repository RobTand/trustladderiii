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

/// Q30 / #806: successful slot scans have spare capacity and disjoint holders.
pub(super) open spec fn slot_safe(w: World, i: int) -> bool {
    w.nodes@[i].repo.0 < w.gates@.len()
        && progress_gate_apply::holders(w, w.nodes@[i].repo, w.nodes@.len())
            < w.gates@[w.nodes@[i].repo.0 as int].w
        && forall|j: int| 0 <= j < w.nodes@.len() && j != i
            && w.nodes@[j].holds && w.nodes@[j].repo == w.nodes@[i].repo
            ==> footprint::disjoint(w, w.nodes@[j].footprint, w.nodes@[i].footprint)
}

/// Q30 / Q39 / #806: real scheduler decisions respect gate and pool capacities.
pub(super) open spec fn context_safe(w: World, i: int, c: AdvanceContext) -> bool {
    (c.pool ==> pool_users(w.nodes@, w.nodes@.len()) < w.pool_capacity)
        && (c.slot && !w.nodes@[i].holds ==> slot_safe(w, i))
        && (c.admit ==> dependencies::delivered(w.dependencies@, NodeId(i as u64))
            && w.nodes@[i].repo.0 < w.gates@.len()
            && slot_waiters(w.nodes@, w.nodes@.len(), w.nodes@[i].repo)
                < w.gates@[w.nodes@[i].repo.0 as int].w)
}

/// #806: blocked dispatch only discards an incident waiter's pending ticket.
pub(super) open spec fn blocked_after(n: Node) -> Node {
    if n.phase is Closed { n }
    else if n.waiting_incident { Node { ticket: None, ..n } }
    else { n }
}

/// #806: isolate blocked framing from the executable rank obligations.
pub(super) proof fn blocked_frame(n: Node, ticket: u64, c: AdvanceContext, out: Outbox)
    requires n.phase is Closed || n.waiting_incident || n.effect.is_some(),
    ensures exact(n, blocked_after(n), ticket, ticket, c, out, out),
{
    reveal(exact); reveal(node_after); reveal(outputs); reveal(effect_kind);
    if n.phase is Closed {} else if n.waiting_incident {} else {}
}

/// Q30 / #806: exact Ready slot update, independent of the rest of the dispatcher.
pub(super) open spec fn slot_after(n: Node, ticket: u64, c: AdvanceContext) -> Node {
    match n.phase {
        Phase::Ready(r) => if n.paused { Node { ticket: None, ..n } }
            else if n.ticket.is_none() { Node { ticket: Some(ticket), ..n } }
            else if c.slot { retirement::phased(Node { holds: true, ..n }, resumed_phase(r)) }
            else { n },
        _ => n,
    }
}

/// Q30 / #806: a slot step preserves the entire output buffer.
pub(super) proof fn slot_frame(n: Node, ticket: u64, c: AdvanceContext, out: Outbox)
    requires advance_pre(n, ticket, out.len),
        !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        current_item(n) == Some(Item::Slot),
    ensures exact(n, slot_after(n, ticket, c), ticket,
        (ticket + if !n.paused && n.ticket.is_none() { 1int } else { 0int }) as u64, c, out, out),
{
    reveal(exact); reveal(node_after); reveal(outputs); reveal(effect_kind);
    assert(n.phase is Ready);
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

/// #806 / #693: successful admission records planning and releases withheld credit.
pub(super) proof fn admission_frame(before: Node, after: Node, ticket_before: u64,
    ticket_after: u64, c: AdvanceContext, out_before: Outbox, out_after: Outbox, moved: bool)
    requires exact(before, after, ticket_before, ticket_after, c, out_before, out_after),
        moved == advance_enabled(before, c), moved,
        before.phase == Phase::Unplanned, before.pos == 0, before.ticket.is_none(),
    ensures after.planning_started, !after.credit.withheld,
        after.credit == (Credit { withheld: false, ..before.credit }),
        after == (Node { credit: Credit { withheld: false, ..before.credit },
            planning_started: true, phase: Phase::Planning { must_split: false },
            generation: (before.generation + 1) as u64, pos: 0, ticket: None, ..before }),
        out_after == out_before, ticket_after == ticket_before,
{
    reveal(exact); reveal(node_after); reveal(outputs); reveal(effect_kind);
}

/// #806 / Q39: a waiting StartRun consumes exactly the scheduler's pool decision.
pub(super) proof fn pool_frame(before: Node, after: Node, ticket_before: u64,
    ticket_after: u64, c: AdvanceContext, out_before: Outbox, out_after: Outbox,
    role: Role, moved: bool)
    requires exact(before, after, ticket_before, ticket_after, c, out_before, out_after),
        moved == advance_enabled(before, c),
        !(before.phase is Closed), !before.waiting_incident, before.effect.is_none(),
        !before.paused, before.ticket.is_some(), current_item(before) == Some(Item::Start(role)),
    ensures moved == c.pool, ticket_after == ticket_before,
        c.pool ==> after == (Node { ticket: None, pool: true, run: Some(before.generation),
            pos: (before.pos + 1) as u8, next_seq: (before.next_seq + 1) as u64,
            effect: Some(InFlight { seq: before.next_seq, kind: EffectKind::StartRun(role),
                cancelling: false }), ..before })
            && appended(out_before, out_after, effect_output(before, c.node, EffectKind::StartRun(role))),
        !c.pool ==> after == before && out_after == out_before,
{
    reveal(exact); reveal(node_after); reveal(outputs); reveal(effect_kind);
}
}
