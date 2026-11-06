//! #806: isolate local rank expansion from exact node and output frames.
use super::port_advance_contracts::{blocked_after, emitted, gone, paused};
use super::*;

verus! {
/// #771 / #806: blocked dispatch changes only an incident waiter's ticket.
pub(super) proof fn blocked(n: Node, ticket: u64, c: AdvanceContext)
    requires advance_pre(n, ticket, 0),
        n.phase is Closed || n.waiting_incident || n.effect.is_some(),
    ensures advance_post(n, blocked_after(n), ticket, ticket, advance_enabled(n, c)),
{
    reveal(advance_post); reveal(distance);
    if n.phase is Closed {} else if n.waiting_incident {} else {}
}

/// Q39 / #771 / #806: prove StartRun ranks without the outbox frame in scope.
pub(super) proof fn start(n: Node, ticket: u64, c: AdvanceContext, role: Role)
    requires advance_pre(n, ticket, 0), !(n.phase is Closed),
        !n.waiting_incident, n.effect.is_none(), current_item(n) == Some(Item::Start(role)),
    ensures advance_post(n,
        if n.paused { paused(n) }
        else if n.ticket.is_none() { Node { ticket: Some(ticket), ..n } }
        else if c.pool { emitted(Node { ticket: None, pool: true,
            run: Some(n.generation), pos: (n.pos + 1) as u8, ..n }, EffectKind::StartRun(role)) }
        else { n },
        ticket, (ticket + if !n.paused && n.ticket.is_none() { 1int } else { 0int }) as u64,
        advance_enabled(n, c)),
{
    assert(n.phase != Phase::Unplanned);
    reveal(advance_post); reveal(distance);
    if n.paused {} else if n.ticket.is_none() {} else if c.pool {} else {}
}

/// Stopping / #771: extract only the bounds needed by the executable cleanup.
pub(super) proof fn teardown_bounds(n: Node, ticket: u64, out_len: usize, kind: EffectKind)
    requires advance_pre(n, ticket, out_len), !(n.phase is Closed),
        !n.waiting_incident, n.effect.is_none(), current_item(n) == Some(Item::Teardown(kind)),
    ensures n.phase is Stopping, n.pos < 5, n.next_seq < u64::MAX, out_len < OUTBOX,
{
}

/// Stopping / #806: a question stutters, with ranks and exact frames proved together.
pub(super) proof fn teardown_question(n: Node, after: Node, ticket: u64, c: AdvanceContext,
    before_out: Outbox, after_out: Outbox, kind: EffectKind)
    requires advance_pre(n, ticket, before_out.len), n.phase is Stopping,
        !n.waiting_incident, n.effect.is_none(), current_item(n) == Some(Item::Teardown(kind)),
        n.question.is_some(), after == n, after_out == before_out,
    ensures advance_post(n, after, ticket, ticket, false), !advance_enabled(n, c),
        port_advance_contracts::exact(n, after, ticket, ticket, c, before_out, after_out),
{
    port_advance_teardown_rank::question(n, ticket, c, kind);
    port_advance_step_frames::teardown_quiet(n, ticket, c, before_out, kind);
}

/// Stopping / #806: skipping a missing resource consumes exactly one agenda entry.
pub(super) proof fn teardown_absent(n: Node, after: Node, ticket: u64, c: AdvanceContext,
    before_out: Outbox, after_out: Outbox, kind: EffectKind)
    requires advance_pre(n, ticket, before_out.len), n.phase is Stopping,
        !n.waiting_incident, n.effect.is_none(), current_item(n) == Some(Item::Teardown(kind)),
        n.question.is_none(), gone(n, kind), after_out == before_out,
        after == (Node { pool: n.pool && kind != EffectKind::StopRun,
            pos: (n.pos + 1) as u8, ..n }),
    ensures advance_post(n, after, ticket, ticket, true), advance_enabled(n, c),
        port_advance_contracts::exact(n, after, ticket, ticket, c, before_out, after_out),
{
    assert(pending_rank(n) == 0 && pending_rank(after) == 0);
    port_advance_teardown_rank::absent(n, ticket, c, kind);
    port_advance_step_frames::teardown_quiet(n, ticket, c, before_out, kind);
}

/// Stopping / #806: compose the real emitter's frame after the effect is allocated.
pub(super) proof fn teardown_emission(n: Node, after: Node, ticket: u64, c: AdvanceContext,
    before_out: Outbox, after_out: Outbox, kind: EffectKind)
    requires advance_pre(n, ticket, before_out.len), n.phase is Stopping,
        !n.waiting_incident, n.effect.is_none(), current_item(n) == Some(Item::Teardown(kind)),
        n.question.is_none(), !gone(n, kind), after == emitted(n, kind),
        after_out.len == before_out.len + 1,
        port_review_output::effect_post(before_out.items@, before_out.len,
            after_out.items@, n, c.node, kind),
    ensures advance_post(n, after, ticket, ticket, true), advance_enabled(n, c),
        port_advance_contracts::exact(n, after, ticket, ticket, c, before_out, after_out),
{
    assert(pending_rank(n) == 0 && pending_rank(after) == 0);
    port_advance_teardown_rank::emission(n, ticket, c, kind);
    port_advance_step_frames::teardown_emit(n, ticket, c, before_out, after_out, kind);
}
}
