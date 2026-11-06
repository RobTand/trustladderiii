//! #806: isolate local rank expansion from exact node and output frames.
use super::port_advance_contracts::{blocked_after, emitted, paused};
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
}
