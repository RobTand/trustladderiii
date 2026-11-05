//! #771/#805: unchanged executable node rank functions, independently gated.
use super::*;

impl Node {
    kernel_fn! {
        /// Bookkeeping settle still owes the node without moving it closer to
        /// done: a move to Ready because it's paused (3), taking a place in line
        /// (2), or giving one back (1).
        pub(super) fn pending(&self) -> (result: u64) [
            ensures result as int == pending_rank(*self), result <= 3,
        ] proof [rank_definitions(*self);] {
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
        ] {
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
        ] proof [rank_definitions(*self); reveal(distance); reveal(effect_distance); reveal(current_item);] {
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
