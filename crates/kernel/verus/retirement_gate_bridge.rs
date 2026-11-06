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
}
