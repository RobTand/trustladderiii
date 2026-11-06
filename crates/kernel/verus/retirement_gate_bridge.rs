//! #806: project W6's shared local transition contract onto the preserved grant API.
use super::*;
verus! {
/// Q30/Q39/Q48: the full local relation includes the original exact Ready grant result.
pub(super) proof fn ready(n: Node, after: Node, ticket: u64, ticket_after: u64,
    c: AdvanceContext, out_before: Outbox, out_after: Outbox)
    requires n.phase == Phase::Ready(Resume::Making), n.pos == 0,
        !n.waiting_incident, n.effect.is_none(),
        port_advance_contracts::exact(n, after, ticket, ticket_after, c, out_before, out_after),
    ensures after == retirement_gate::slot_node(n, ticket, c.slot),
        ticket_after == retirement_gate::slot_ticket(n, ticket),
        out_after == out_before,
        advance_enabled(n, c) == retirement_gate::slot_changed(n, c.slot),
{
    reveal(port_advance_contracts::exact); reveal(port_advance_contracts::node_after);
    reveal(port_advance_contracts::outputs); reveal(port_advance_contracts::effect_kind);
    reveal(retirement_gate::slot_node);
}

/// Q30 / #806: the two recursive holder scans count the same prefix.
pub(super) proof fn count(w: World, i: int, end: nat)
    ensures retirement_gate::slot_count(w, i, end) == port_advance_gate::gate_slot_count(w, i, end),
    decreases end,
{
    if end > 0 { count(w, i, (end - 1) as nat); }
}
/// Q30 / Q48 / #806: bridge neutral scan predicates without changing the reviewed API.
pub(super) proof fn queries(w: World, i: int)
    ensures retirement_gate::slot(w, i) == port_advance_gate::gate_slot(w, i),
        retirement_gate::line(w, i) == port_advance_gate::gate_line(w, i),
{
    reveal(port_advance_gate::gate_slot); reveal(port_advance_gate::gate_line);
    reveal(port_advance_gate::gate_overlap);
    count(w, i, w.nodes@.len());
    assert forall|j: int| 0 <= j < w.nodes@.len() implies
        retirement_gate::slot(w, j) == port_advance_gate::gate_slot(w, j)
        && retirement_gate::eligible(w, i, j) == port_advance_gate::gate_eligible(w, i, j) by {
        count(w, j, w.nodes@.len());
        assert forall|t: u64| retirement_gate::overlap(w, j, t)
            == port_advance_gate::gate_overlap(w, j, t) by {}
    }
    assert forall|j: int| 0 <= j < w.nodes@.len() implies
        retirement_gate::older(w, i, j) == port_advance_gate::gate_older(w, i, j) by {}

}
/// Q30 / Q39 / #806: project W6's proven scheduler postcondition onto the stable grant API.
pub(super) proof fn post(n: Node, m: Node, ticket: u64, next: u64, grant: bool,
    changed: bool, before: Outbox, after: Outbox)
    requires port_advance_gate::gate_ready_post(n, m, ticket, next, grant, changed, before, after),
    ensures m == retirement_gate::slot_node(n, ticket, grant),
        next == retirement_gate::slot_ticket(n, ticket), after == before,
        changed == retirement_gate::slot_changed(n, grant),
{
    reveal(port_advance_gate::gate_slot_node); reveal(retirement_gate::slot_node);
}
}
