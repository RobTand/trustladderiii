//! Q30/Q39 / #806: real slot transition, independently gated for uncached time.
use super::*;

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    pub(super) fn advance_slot(node: &mut Node, next_ticket: &mut u64,
        context: AdvanceContext, _out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(next_ticket), old(_out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Slot),
        ensures (!result ==> final(_out).len == old(_out).len),
            advance_post(*old(node), *final(node),
            *old(next_ticket), *final(next_ticket), result),
            result == advance_enabled(*old(node), context),
            old(node).phase == Phase::Ready(Resume::Making) ==> (*final(node) == retirement_gate::slot_node(*old(node), *old(next_ticket), context.slot)),
            old(node).phase == Phase::Ready(Resume::Making) ==> *final(next_ticket) == retirement_gate::slot_ticket(*old(node), *old(next_ticket)),
            *final(_out) == *old(_out),
            result == retirement_gate::slot_changed(*old(node), context.slot),
    ] {
        kernel_snapshot! { before = *node }
        kernel_snapshot! { ticket = *next_ticket }
        let result = advance_slot_update(node, next_ticket, context, _out);
        kernel_proof! { slot_progress(before@, ticket@, context); }
        result
    }
}

kernel_fn! {
/// Q30/Q39: unchanged executable writes, separated from progress arithmetic.
fn advance_slot_update(node: &mut Node, next_ticket: &mut u64,
    context: AdvanceContext, _out: &mut Outbox) -> (result: bool) [
    requires advance_pre(*old(node), *old(next_ticket), old(_out).len),
        !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none()
            && current_item(*old(node)) == Some(Item::Slot),
    ensures *final(node) == retirement_gate::slot_node(*old(node), *old(next_ticket), context.slot),
        *final(next_ticket) == retirement_gate::slot_ticket(*old(node), *old(next_ticket)),
        result == retirement_gate::slot_changed(*old(node), context.slot),
        result == advance_enabled(*old(node), context),
        *final(_out) == *old(_out),
    ] proof [reveal(retirement_gate::slot_node);] {
        let n = *node;
        let Phase::Ready(r) = n.phase else {
            return false;
        };
        if n.paused {
            if n.ticket.is_some() {
                node.ticket = None;
                return true;
            }
            return false;
        }
        if n.ticket.is_none() {
            take_ticket_node(node, next_ticket);
            return true;
        }
        if context.slot {
            let m = &mut *node;
            m.holds = true;
            set_phase(m, r.phase());
            return true;
        }
        false
    }
}

#[cfg(verus_keep_ghost)]
verus! {
/// Q30/Q39/#805/#806: the exact slot update implies the complete original
/// progress contract for every input allowed at the executable call site.
proof fn slot_progress(n: Node, ticket: u64, context: AdvanceContext)
    requires advance_pre(n, ticket, 0),
        !(n.phase is Closed) && !n.waiting_incident && n.effect.is_none()
            && current_item(n) == Some(Item::Slot),
    ensures advance_post(n, retirement_gate::slot_node(n, ticket, context.slot),
        ticket, retirement_gate::slot_ticket(n, ticket), retirement_gate::slot_changed(n, context.slot)),
{
    reveal(advance_post); reveal(distance); reveal(retirement_gate::slot_node);
    rank_definitions(n);
    let after = retirement_gate::slot_node(n, ticket, context.slot);
    rank_definitions(after);
    match n.phase {
        Phase::Ready(_) => {
            if n.paused {} else if n.ticket.is_none() {} else if context.slot {} else {}
        },
        _ => {},
    }
}
}
