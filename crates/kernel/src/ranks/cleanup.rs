//! #771: unchanged local transition contracts and executable bodies.
use super::*;

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    pub(super) fn advance_rebase(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::StartRebase),
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
        m.run = Some(m.generation);
        m.pos += 1;
        emit_node(node, context.node, EffectKind::StartRebase, out);
        true
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    pub(super) fn advance_admit(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, _out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(_out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Admit),
        ensures (!result ==> final(_out).len == old(_out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [reveal(advance_post); reveal(distance);] {
        let n = *node;
        if n.paused || !context.admit {
            return false;
        }
        let m = &mut *node;
        m.credit.withheld = false;
        m.planning_started = true;
        set_phase(m, Phase::Planning { must_split: false });
        true
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    pub(super) fn advance_release(node: &mut Node, _next_ticket: &mut u64,
        _context: AdvanceContext, _out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(_out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Release),
        ensures (!result ==> final(_out).len == old(_out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), _context),
    ] proof [reveal(advance_post); reveal(distance);] {
        let m = &mut *node;
        m.holds = false;
        m.pos += 1;
        true
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    pub(super) fn advance_teardown(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox, kind: EffectKind) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Teardown(kind)),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [reveal(advance_post);] {
        let n = *node;
        if n.question.is_some() {
            return false; // waiting for Rob
        }
        let gone = match kind {
            EffectKind::StopRun => n.run.is_none(),
            EffectKind::WithdrawJob => n.job.is_none(),
            EffectKind::ClosePr => !n.pr_open,
            EffectKind::RemoveWorktree => !n.worktree,
            _ => false,
        };
        if gone {
            return skip_teardown(node, _next_ticket, context, out, kind);
        }
        emit_teardown(node, _next_ticket, context, out, kind)
    }
}
kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    fn skip_teardown(node: &mut Node, _next_ticket: &mut u64,
        _context: AdvanceContext, _out: &mut Outbox, kind: EffectKind) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(_out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Teardown(kind)),
            old(node).question.is_none(),
        ensures result, (!result ==> final(_out).len == old(_out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), _context),
    ] proof [reveal(advance_post); reveal(distance);] {
        let m = &mut *node;
        if kind == EffectKind::StopRun {
            m.pool = false;
        }
        m.pos += 1;
        true
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    fn emit_teardown(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox, kind: EffectKind) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Teardown(kind)),
            old(node).question.is_none(),
        ensures result, (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [reveal(advance_post); reveal(distance);] {
        emit_node(node, context.node, kind, out);
        true
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    pub(super) fn advance_idle(_node: &mut Node, _next_ticket: &mut u64,
        _context: AdvanceContext, _out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(_node), *old(_next_ticket), old(_out).len),
            !(old(_node).phase is Closed) && !old(_node).waiting_incident && old(_node).effect.is_none() && matches!(current_item(*old(_node)), Some(Item::Await | Item::Merge | Item::Answer)),
        ensures (!result ==> final(_out).len == old(_out).len),
            advance_post(*old(_node), *final(_node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(_node), _context),
    ] proof [reveal(advance_post); reveal(distance);] {
        false
    }
}
