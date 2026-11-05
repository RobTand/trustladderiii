//! #771: unchanged local transition contracts and executable bodies.
use super::*;

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    pub(super) fn advance_blocked(node: &mut Node, _next_ticket: &mut u64,
        _context: AdvanceContext, _out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(_out).len),
            old(node).phase is Closed || old(node).waiting_incident || old(node).effect.is_some(),
        ensures (!result ==> final(_out).len == old(_out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), _context),
    ] proof [reveal(advance_post); reveal(distance);] {
        let n = *node;
        if n.phase.is_closed() {
            return false;
        }
        if n.waiting_incident {
            if n.ticket.is_some() {
                node.ticket = None;
                return true;
            }
            return false;
        }
        if n.effect.is_some() {
            return false; // one effect in flight at a time
        }
        false
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    pub(super) fn advance_finished(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)).is_none(),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [reveal(advance_post); reveal(distance);] {
        let n = *node;
        match n.phase {
            Phase::Publishing => {
                set_phase(node, Phase::Queued);
                true
            }
            Phase::Stopping(outcome) if n.question.is_none() => {
                close_node(node, context.node, outcome, out);
                true
            }
            _ => false,
        }
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    pub(super) fn advance_do(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox, kind: EffectKind) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Do(kind)),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [reveal(advance_post); reveal(distance);] {
        let n = *node;
        if n.paused {
            return pause_node(node);
        }
        emit_node(node, context.node, kind, out);
        true
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    pub(super) fn advance_start(node: &mut Node, next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox, role: Role) -> (result: bool) [
        requires advance_pre(*old(node), *old(next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Start(role)),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(next_ticket), *final(next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [reveal(advance_post); reveal(distance);] {
        let n = *node;
        if n.paused {
            return pause_node(node);
        }
        if n.ticket.is_none() {
            take_ticket_node(node, next_ticket);
            return true;
        }
        if context.pool {
            let m = &mut *node;
            m.ticket = None;
            m.pool = true;
            m.run = Some(m.generation);
            m.pos += 1;
            emit_node(node, context.node, EffectKind::StartRun(role), out);
            return true;
        }
        false
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    pub(super) fn advance_checks(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::StartChecks),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [reveal(advance_post); reveal(distance);] {
        let n = *node;
        if n.paused {
            return pause_node(node);
        }
        let m = &mut *node;
        m.job = Some(m.generation);
        m.pos += 1;
        emit_node(node, context.node, EffectKind::SubmitChecks, out);
        true
    }
}
