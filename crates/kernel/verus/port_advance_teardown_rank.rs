//! #806: independent teardown rank queries with the original pending-rank facts supplied.
use super::port_advance_contracts::{emitted, gone};
use super::*;

verus! {
/// Stopping / #771 / #806: compose teardown ranks before adding output facts.
pub(super) proof fn step(n: Node, ticket: u64, c: AdvanceContext, kind: EffectKind)
    requires advance_pre(n, ticket, 0), !(n.phase is Closed),
        !n.waiting_incident, n.effect.is_none(), current_item(n) == Some(Item::Teardown(kind)),
    ensures advance_post(n,
        if n.question.is_some() { n }
        else if gone(n, kind) { Node { pool: n.pool && kind != EffectKind::StopRun,
            pos: (n.pos + 1) as u8, ..n } }
        else { emitted(n, kind) }, ticket, ticket, advance_enabled(n, c)),
{
    let after = if n.question.is_some() { n }
        else if gone(n, kind) { Node { pool: n.pool && kind != EffectKind::StopRun,
            pos: (n.pos + 1) as u8, ..n } }
        else { emitted(n, kind) };
    assert(n.phase is Stopping);
    assert(pending_rank(n) == 0 && pending_rank(after) == 0);
    if n.question.is_some() { question(n, ticket, c, kind); }
    else if gone(n, kind) { absent(n, ticket, c, kind); }
    else { emission(n, ticket, c, kind); }
}

/// Stopping / #806: an open question stutters in every rank and capacity.
pub(super) proof fn question(n: Node, ticket: u64, c: AdvanceContext, kind: EffectKind)
    requires n.phase is Stopping, !n.waiting_incident, n.effect.is_none(),
        current_item(n) == Some(Item::Teardown(kind)), n.question.is_some(),
    ensures advance_post(n, n, ticket, ticket, advance_enabled(n, c)),
{
    reveal(advance_post);
}

/// Stopping / #806: skipping an absent resource strictly consumes its agenda entry.
pub(super) proof fn absent(n: Node, ticket: u64, c: AdvanceContext, kind: EffectKind)
    requires advance_pre(n, ticket, 0), n.phase is Stopping,
        !n.waiting_incident, n.effect.is_none(), n.question.is_none(),
        current_item(n) == Some(Item::Teardown(kind)), gone(n, kind),
        pending_rank(n) == 0,
        pending_rank(Node { pool: n.pool && kind != EffectKind::StopRun,
            pos: (n.pos + 1) as u8, ..n }) == 0,
    ensures advance_post(n, Node { pool: n.pool && kind != EffectKind::StopRun,
        pos: (n.pos + 1) as u8, ..n }, ticket, ticket, advance_enabled(n, c)),
{
    reveal(advance_post); reveal(distance);
    assert(n.pos < 5);
}

/// Stopping / #806: emitting teardown reduces distance by exactly one.
pub(super) proof fn emission(n: Node, ticket: u64, c: AdvanceContext, kind: EffectKind)
    requires advance_pre(n, ticket, 0), n.phase is Stopping,
        !n.waiting_incident, n.effect.is_none(), n.question.is_none(),
        current_item(n) == Some(Item::Teardown(kind)), !gone(n, kind),
        pending_rank(n) == 0, pending_rank(emitted(n, kind)) == 0,
    ensures advance_post(n, emitted(n, kind), ticket, ticket, advance_enabled(n, c)),
{
    reveal(advance_post); reveal(distance);
    assert(n.pos < 5);
}
}
