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
    ] proof [reveal(advance_post); reveal(distance); ] {
        kernel_proof! { if node.phase == Phase::Ready(Resume::Making) { reveal(retirement_gate::slot_node); } }
        kernel_proof! { rank_definitions(*node); }
        let n = *node;
        let Phase::Ready(r) = n.phase else {
            return false;
        };
        if n.paused {
            if n.ticket.is_some() {
                node.ticket = None;
                kernel_proof! { rank_definitions(*node); }
                return true;
            }
            return false;
        }
        if n.ticket.is_none() {
            take_ticket_node(node, next_ticket);
            kernel_proof! { rank_definitions(*node); }
            return true;
        }
        if context.slot {
            let m = &mut *node;
            m.holds = true;
            set_phase(m, r.phase());
            kernel_proof! { rank_definitions(*node); }
            return true;
        }
        false
    }
}
