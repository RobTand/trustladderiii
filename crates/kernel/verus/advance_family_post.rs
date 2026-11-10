//! G5: exact Slot-arm update, isolated from world-level safety reasoning.
use super::*;

verus! {
pub(super) open spec fn same(b: Node, n: Node, t0: u64, t1: u64,
    o0: Outbox, o1: Outbox, moved: bool) -> bool {
    n == b && t1 == t0 && !moved
        && o1.len == o0.len && o1.items@ == o0.items@
}

pub(super) open spec fn issued(b: Node, n: Node, t0: u64, t1: u64,
    o0: Outbox, o1: Outbox, moved: bool) -> bool {
    n == (Node { ticket: Some(t0), ..b }) && t1 as int == t0 as int + 1
        && moved && o1.len == o0.len && o1.items@ == o0.items@
}

#[verifier::opaque]
pub(super) open spec fn slot(b: Node, n: Node, t0: u64, t1: u64, c: AdvanceContext,
    o0: Outbox, o1: Outbox, moved: bool) -> bool {
    match b.phase {
        Phase::Ready(r) => if b.paused {
            moved == b.ticket.is_some() && n == (Node { ticket: None, ..b })
                && t1 == t0 && o1.len == o0.len && o1.items@ == o0.items@
        } else if b.ticket.is_none() {
            issued(b, n, t0, t1, o0, o1, moved)
        } else if c.slot {
            moved && t1 == t0 && o1.len == o0.len && o1.items@ == o0.items@
                && n == retirement::phased(Node { holds: true, ..b }, resumed_phase(r))
        } else { same(b, n, t0, t1, o0, o1, moved) },
        _ => same(b, n, t0, t1, o0, o1, moved),
    }
}
}
