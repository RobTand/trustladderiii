//! #806 W7: required admission and pool assertions across the E-owned rank boundary.
use super::*;

verus! {
/// State-table prerequisite delivery / #806: real admission releases withheld
/// credit and marks planning as started. This is the original delivery cover.
pub(super) fn admission_contract_gap(mut node: Node, mut next_ticket: u64) -> (result: Node)
    requires advance_pre(node, next_ticket, 0),
        node.phase == Phase::Unplanned, node.pos == 0, node.ticket.is_none(),
        !node.paused, !node.waiting_incident, node.effect.is_none(),
    ensures result.phase == (Phase::Planning { must_split: false }),
        result.planning_started, !result.credit.withheld,
{
    let mut out = Outbox::new();
    proof { reveal(empty_outbox); }
    let changed = advance_node(&mut node, &mut next_ticket,
        AdvanceContext { node: 0, admit: true, pool: false, slot: false }, &mut out);
    assert(changed);
    node
}

/// State-table pool grant / Q39 / #806: the first eligible pool waiter receives
/// the grant and consumes its ticket through the real local scheduler transition.
pub(super) fn pool_contract_gap(mut node: Node, mut next_ticket: u64) -> (result: Node)
    requires advance_pre(node, next_ticket, 0),
        node.phase == Phase::Ingesting, node.pos == 0, node.ticket.is_some(),
        !node.paused, !node.waiting_incident, node.effect.is_none(),
    ensures result.pool, result.ticket.is_none(),
{
    let mut out = Outbox::new();
    proof { reveal(empty_outbox); }
    let changed = advance_node(&mut node, &mut next_ticket,
        AdvanceContext { node: 0, admit: false, pool: true, slot: false }, &mut out);
    assert(changed);
    node
}
}
