//! Q30 / #806: separate slot rank arithmetic from the executable local frame.
use super::*;

verus! {
/// Q30 / #806: Ready's exact step retains all previously verified rank bounds.
pub(super) proof fn rank(n: Node, ticket: u64, c: AdvanceContext, out_len: usize)
    requires advance_pre(n, ticket, out_len),
        !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        current_item(n) == Some(Item::Slot),
    ensures advance_post(n, port_advance_contracts::slot_after(n, ticket, c), ticket,
        (ticket + if !n.paused && n.ticket.is_none() { 1int } else { 0int }) as u64,
        advance_enabled(n, c)),
{
    reveal(advance_post); reveal(distance);
    assert(n.phase is Ready);
    let r = match n.phase { Phase::Ready(r) => r, _ => Resume::Making };
    let after = port_advance_contracts::slot_after(n, ticket, c);
    let ticket_after = (ticket + if !n.paused && n.ticket.is_none() { 1int } else { 0int }) as u64;
    if n.paused {
        assert(advance_post(n, after, ticket, ticket_after, advance_enabled(n, c)));
    } else if n.ticket.is_none() {
        assert(advance_post(n, after, ticket, ticket_after, advance_enabled(n, c)));
    } else if c.slot {
        match r {
            Resume::Making => { assert(advance_post(n, after, ticket, ticket_after, true)); },
            Resume::Checking => { assert(advance_post(n, after, ticket, ticket_after, true)); },
            Resume::Reviewing => { assert(advance_post(n, after, ticket, ticket_after, true)); },
            Resume::Publishing => { assert(advance_post(n, after, ticket, ticket_after, true)); },
            Resume::Rebasing => { assert(advance_post(n, after, ticket, ticket_after, true)); },
        }
    } else {
        assert(advance_post(n, after, ticket, ticket_after, false));
    }
}
}
