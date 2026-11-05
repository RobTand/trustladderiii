//! Mathematical ranks for the executable state-table code, #771.
//! These definitions contain no transition relation. Executable rank functions
//! prove their contracts against these mathematical values.
use super::*;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(verus_keep_ghost)]
pub(super) use super::progress_gate_leaf_ranks::*;

impl Node {
    kernel_fn! {
        /// Bookkeeping settle still owes the node without moving it closer to
        /// done: a move to Ready because it's paused (3), taking a place in line
        /// (2), or giving one back (1).
        fn pending(&self) -> (result: u64) [
            ensures result as int == pending_rank(*self), result <= 3,
        ] proof [progress_gate_leaf_ranks::rank_definitions_all();] {
            if self.phase.is_closed() {
                return 0;
            }
            if self.waiting_incident {
                return if self.ticket.is_some() { 1 } else { 0 };
            }
            if self.effect.is_some() {
                return 0;
            }
            if self.pause_move_due() {
                return 3;
            }
            match self.current() {
                Some(Item::Start(_) | Item::Slot) => match (self.paused, self.ticket) {
                    (false, None) => 2,
                    (true, Some(_)) => 1,
                    _ => 0,
                },
                _ => 0,
            }
        }
    }

    kernel_fn! {
        /// Falls with every internal transition settle takes. Kani proves it, so
        /// settle ends for any number of requests.
        pub fn settle_rank(&self) -> (result: u64) [
            ensures result as int == 4 * node_distance(*self) + pending_rank(*self),
                result <= 16387,
        ] proof [progress_gate_leaf_ranks::rank_definitions_all();] {
            4 * self.distance() + self.pending()
        }
    }

    kernel_fn! {
        /// Distance to Closed. Each witnessed event lowers the world's total, and
        /// no internal transition raises it.
        // The pinned Verus frontend requires nested if-let instead of let chains.
        #[allow(clippy::collapsible_if)]
        pub fn distance(&self) -> (result: u64) [
            ensures result as int == node_distance(*self), result <= 4096,
        ] proof [progress_gate_leaf_ranks::rank_definitions_all(); reveal(distance); reveal(effect_distance); reveal(current_item);] {
            if self.phase.is_closed() {
                return 0;
            }
            if self.pause_move_due() {
                if let Some(r) = self.phase.resume() {
                    // Counted as the paused Ready it is about to become.
                    return rank(Phase::Ready(r)) * 64 + 2 + 8;
                }
            }
            kernel_proof! { assert(!pause_due(*self)); }
            let rank = match self.phase {
                Phase::AwaitingRob => match self.question {
                    Some(q) => cont_rank(q.cont) + 1,
                    None => 3,
                },
                phase => rank(phase),
            };
            let len = agenda(self.phase).len() as u64;
            let pos = (self.pos as u64).min(len);
            let mut d = rank * 64 + 2 * (len - pos);
            kernel_proof! { assert(d as int == base_distance(*self)); }
            if let Some(e) = self.effect {
                d += 1;
                if !e.cancelling {
                    if let Some(Item::Do(k) | Item::Teardown(k)) = self.current() {
                        if k == e.kind {
                            d -= 2;
                        }
                    }
                }
            }
            kernel_proof! {
                assert(d as int == base_distance(*self) + effect_distance(*self));
            }
            if self.paused && !self.phase.is_stopping() {
                d += 8;
            }
            if self.waiting_incident {
                d += 8;
            }
            if self.question.is_some() && self.phase.is_stopping() {
                d += 4;
            }
            d
        }
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    fn advance_blocked(node: &mut Node, _next_ticket: &mut u64,
        _context: AdvanceContext, _out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(_out).len),
            old(node).phase is Closed || old(node).waiting_incident || old(node).effect.is_some(),
        ensures (!result ==> final(_out).len == old(_out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), _context),
    ] proof [progress_gate_leaf_ranks::rank_definitions_all(); reveal(advance_post); reveal(distance);] {
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
    fn advance_finished(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)).is_none(),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
            progress_gate_leaf_steps::advance_details(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket)),
    ] proof [progress_gate_leaf_ranks::rank_definitions_all(); reveal(advance_post); reveal(distance); reveal(progress_gate_leaf_steps::advance_details);] {
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
    fn advance_do(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox, kind: EffectKind) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Do(kind)),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
            progress_gate_leaf_steps::advance_details(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket)),
    ] proof [progress_gate_leaf_ranks::rank_definitions_all(); reveal(advance_post); reveal(distance); reveal(progress_gate_leaf_steps::advance_details);] {
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
    fn advance_start(node: &mut Node, next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox, role: Role) -> (result: bool) [
        requires advance_pre(*old(node), *old(next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Start(role)),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(next_ticket), *final(next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [progress_gate_leaf_ranks::rank_definitions_all(); reveal(advance_post); reveal(distance);] {
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
    fn advance_checks(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::StartChecks),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [progress_gate_leaf_ranks::rank_definitions_all(); reveal(advance_post); reveal(distance);] {
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

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    fn advance_rebase(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::StartRebase),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [progress_gate_leaf_ranks::rank_definitions_all(); reveal(advance_post); reveal(distance);] {
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
    fn advance_admit(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, _out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(_out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Admit),
        ensures (!result ==> final(_out).len == old(_out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [progress_gate_leaf_ranks::rank_definitions_all(); reveal(advance_post); reveal(distance);] {
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
    fn advance_release(node: &mut Node, _next_ticket: &mut u64,
        _context: AdvanceContext, _out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(_out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Release),
        ensures (!result ==> final(_out).len == old(_out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), _context),
            progress_gate_leaf_steps::advance_details(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket)),
    ] proof [progress_gate_leaf_ranks::rank_definitions_all(); reveal(advance_post); reveal(distance); reveal(progress_gate_leaf_steps::advance_details);] {
        let m = &mut *node;
        m.holds = false;
        m.pos += 1;
        true
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    #[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
    fn advance_teardown(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox, kind: EffectKind) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Teardown(kind)),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
            progress_gate_leaf_steps::advance_details(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket)),
    ] proof [progress_gate_leaf_ranks::rank_definitions_all();] {
        let n = *node;
        kernel_proof! { progress_gate_leaf_steps::teardown_item(n, kind); }
        if n.question.is_some() {
            kernel_proof! {
                progress_gate_leaf_steps::stutter_rank(n, *_next_ticket);
                progress_gate_leaf_steps::inactive_details(n, *node, *_next_ticket, *_next_ticket);
            }
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
            let m = &mut *node;
            if kind == EffectKind::StopRun {
                m.pool = false;
            }
            m.pos += 1;
            kernel_proof! {
                progress_gate_leaf_steps::teardown_result(n, *node, kind, *_next_ticket);
            }
            return true;
        }
        emit_node(node, context.node, kind, out);
        kernel_proof! {
            progress_gate_leaf_steps::teardown_result(n, *node, kind, *_next_ticket);
        }
        true
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    fn advance_idle(_node: &mut Node, _next_ticket: &mut u64,
        _context: AdvanceContext, _out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(_node), *old(_next_ticket), old(_out).len),
            !(old(_node).phase is Closed) && !old(_node).waiting_incident && old(_node).effect.is_none() && matches!(current_item(*old(_node)), Some(Item::Await | Item::Merge | Item::Answer)),
        ensures (!result ==> final(_out).len == old(_out).len),
            advance_post(*old(_node), *final(_node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(_node), _context),
    ] proof [progress_gate_leaf_ranks::rank_definitions_all(); reveal(advance_post); reveal(distance);] {
        false
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    pub(super) fn advance_node(node: &mut Node, next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(next_ticket), old(out).len),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(next_ticket), *final(next_ticket), result),
            result == advance_enabled(*old(node), context),
            (old(node).phase == Phase::Ready(Resume::Making) && old(node).pos == 0 && !old(node).waiting_incident && old(node).effect.is_none()) ==> {
                *final(node) == retirement_gate::slot_node(*old(node), *old(next_ticket), context.slot)
                    && *final(next_ticket) == retirement_gate::slot_ticket(*old(node), *old(next_ticket))
                    && *final(out) == *old(out)
                    && result == retirement_gate::slot_changed(*old(node), context.slot)
            },
            progress_gate_leaf_steps::advance_details(*old(node), *final(node),
            *old(next_ticket), *final(next_ticket)),
    ] proof [progress_gate_leaf_ranks::rank_definitions_all();] {
        kernel_proof! { reveal(progress_gate_leaf_steps::advance_details); }
        let n = *node;
        if n.phase.is_closed() || n.waiting_incident || n.effect.is_some() {
            return advance_blocked(node, next_ticket, context, out);
        }
        let Some(item) = n.current() else {
            return advance_finished(node, next_ticket, context, out);
        };
        match item {
            Item::Do(kind) => advance_do(node, next_ticket, context, out, kind),
            Item::Start(role) => advance_start(node, next_ticket, context, out, role),
            Item::StartChecks => advance_checks(node, next_ticket, context, out),
            Item::StartRebase => advance_rebase(node, next_ticket, context, out),
            Item::Await | Item::Merge | Item::Answer => advance_idle(node, next_ticket, context, out),
            Item::Admit => advance_admit(node, next_ticket, context, out),
            Item::Slot => advance_slot(node, next_ticket, context, out),
            Item::Release => advance_release(node, next_ticket, context, out),
            Item::Teardown(kind) => advance_teardown(node, next_ticket, context, out, kind),
        }
    }
}
