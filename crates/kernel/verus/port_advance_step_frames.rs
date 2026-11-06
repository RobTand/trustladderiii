//! #806: small exact step frames, separate from executable rank arithmetic.
use super::port_advance_contracts::*;
use super::*;

verus! {
/// Q39 / #806: StartRun's paused, ticket-allocation and nongrant branches emit nothing.
pub(super) proof fn start_quiet(n: Node, ticket: u64, c: AdvanceContext, out: Outbox)
    requires advance_pre(n, ticket, out.len), !(n.phase is Closed),
        !n.waiting_incident, n.effect.is_none(), matches!(current_item(n), Some(Item::Start(_))),
    ensures
        n.paused ==> exact(n, paused(n), ticket, ticket, c, out, out),
        !n.paused && n.ticket.is_none() ==> exact(n, Node { ticket: Some(ticket), ..n },
            ticket, (ticket + 1) as u64, c, out, out),
        !n.paused && n.ticket.is_some() && !c.pool ==> exact(n, n, ticket, ticket, c, out, out),
{
    reveal(exact); reveal(node_after); reveal(outputs); reveal(effect_kind);
}

/// Q39 / #806: compose the exact StartRun effect slot with its local node update.
pub(super) proof fn start_emit(n: Node, ticket: u64, c: AdvanceContext, role: Role,
    before: Outbox, after: Outbox)
    requires !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        current_item(n) == Some(Item::Start(role)), !n.paused, n.ticket.is_some(), c.pool,
        after.len == before.len + 1,
        port_review_output::effect_post(before.items@, before.len, after.items@,
            Node { ticket: None, pool: true, run: Some(n.generation), pos: (n.pos + 1) as u8, ..n },
            c.node, EffectKind::StartRun(role)),
    ensures exact(n, emitted(Node { ticket: None, pool: true, run: Some(n.generation),
        pos: (n.pos + 1) as u8, ..n }, EffectKind::StartRun(role)), ticket, ticket, c, before, after),
{
    reveal(exact); reveal(node_after); reveal(outputs); reveal(effect_kind);
    reveal(port_review_output::effect_post); reveal(port_review_output::selected_head);
}

/// Stopping / #806: pending questions and already absent resources have no output.
pub(super) proof fn teardown_quiet(n: Node, ticket: u64, c: AdvanceContext,
    out: Outbox, kind: EffectKind)
    requires !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        current_item(n) == Some(Item::Teardown(kind)),
    ensures n.question.is_some() ==> exact(n, n, ticket, ticket, c, out, out),
        n.question.is_none() && gone(n, kind) ==> exact(n,
            Node { pool: n.pool && kind != EffectKind::StopRun, pos: (n.pos + 1) as u8, ..n },
            ticket, ticket, c, out, out),
{
    reveal(exact); reveal(node_after); reveal(outputs); reveal(effect_kind);
}

/// Stopping / #806: a required teardown emits exactly its original-node effect slot.
pub(super) proof fn teardown_emit(n: Node, ticket: u64, c: AdvanceContext,
    before: Outbox, after: Outbox, kind: EffectKind)
    requires !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        current_item(n) == Some(Item::Teardown(kind)), n.question.is_none(), !gone(n, kind),
        after.len == before.len + 1,
        port_review_output::effect_post(before.items@, before.len, after.items@, n, c.node, kind),
    ensures exact(n, emitted(n, kind), ticket, ticket, c, before, after),
{
    reveal(exact); reveal(node_after); reveal(outputs); reveal(effect_kind);
    reveal(port_review_output::effect_post); reveal(port_review_output::selected_head);
}

/// Q30 / #806: Release advances one agenda entry and preserves the full outbox.
pub(super) proof fn release_frame(n: Node, ticket: u64, c: AdvanceContext, out: Outbox)
    requires !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        current_item(n) == Some(Item::Release),
    ensures exact(n, Node { holds: false, pos: (n.pos + 1) as u8, ..n },
        ticket, ticket, c, out, out),
{
    reveal(exact); reveal(node_after); reveal(outputs); reveal(effect_kind);
}
/// Publishing/Stopping / #806: finished branches without closure preserve all outputs.
pub(super) proof fn finished_quiet(n: Node, ticket: u64, c: AdvanceContext, out: Outbox)
    requires !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        current_item(n).is_none(),
    ensures n.phase == Phase::Publishing ==> exact(n, retirement::phased(n, Phase::Queued),
            ticket, ticket, c, out, out),
        n.phase != Phase::Publishing && (!(n.phase is Stopping) || n.question.is_some())
            ==> exact(n, n, ticket, ticket, c, out, out),
{
    reveal(exact); reveal(node_after); reveal(outputs); reveal(effect_kind);
}

/// Stopping / #806: compose close_node's ordered exact notices without rank arithmetic.
pub(super) proof fn finished_close(n: Node, ticket: u64, c: AdvanceContext,
    before: Outbox, after: Outbox, outcome: Outcome)
    requires n.phase == Phase::Stopping(outcome), !n.waiting_incident,
        n.effect.is_none(), n.question.is_none(), current_item(n).is_none(),
        after.len == before.len + if outcome == Outcome::Replaced { 2usize } else { 1usize },
        after.items@ == if outcome == Outcome::Replaced {
            before.items@.update(before.len as int,
                Some(Out::Notice { node: NodeId(c.node as u64), notice: Notice::Summary(outcome) }))
                .update(before.len as int + 1,
                    Some(Out::Notice { node: NodeId(c.node as u64), notice: Notice::FileReplacement }))
        } else { before.items@.update(before.len as int,
            Some(Out::Notice { node: NodeId(c.node as u64), notice: Notice::Summary(outcome) })) },
    ensures exact(n, closed(n, outcome), ticket, ticket, c, before, after),
{
    reveal(exact); reveal(node_after); reveal(outputs); reveal(effect_kind);
}

}
