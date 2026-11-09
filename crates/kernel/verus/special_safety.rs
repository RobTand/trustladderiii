//! K4c: concept lemmas replacing the Kani SPECIAL harnesses, over arbitrary
//! nodes and worlds rather than the harnesses' bounded fixtures.
use super::*;

verus! {
/// Replaces Kani `apply_rebase_progress_gate`. A Rebasing request awaits its
/// rebase reply exactly at agenda position 1, after StartRebase at position 0,
/// so the rebase fuel rule applies only to a reply the request is awaiting.
pub(super) proof fn rebase_await_gate(n: Node)
    requires n.phase == Phase::Rebasing,
    ensures (current_item(n) == Some(Item::Await)) == (n.pos == 1),
        (current_item(n) == Some(Item::StartRebase)) == (n.pos == 0),
        n.pos >= 2 ==> current_item(n).is_none(),
{
}

/// Replaces the order assertion of Kani `pool_grant_is_priority_ordered` for
/// arbitrary worlds. A pool waiter (a non-Ready ticket holder) is first in
/// line exactly when no other waiter has a better priority, or the same
/// priority and an older ticket.
pub(super) proof fn pool_line_order(w: World, i: int)
    requires 0 <= i < w.nodes@.len(), !(w.nodes@[i].phase is Ready),
        w.nodes@[i].ticket.is_some(),
    ensures oldest_waiter(w, i) == forall|j: int| 0 <= j < w.nodes@.len()
        && !(w.nodes@[j].phase is Ready) && w.nodes@[j].ticket.is_some()
        ==> !(priority_number(w.nodes@[j].priority) < priority_number(w.nodes@[i].priority)
            || (w.nodes@[j].priority == w.nodes@[i].priority
                && w.nodes@[j].ticket.unwrap() < w.nodes@[i].ticket.unwrap())),
{
    let me = w.nodes@[i];
    if oldest_waiter(w, i) {
        assert forall|j: int| 0 <= j < w.nodes@.len()
            && !(w.nodes@[j].phase is Ready) && w.nodes@[j].ticket.is_some()
            implies !(priority_number(w.nodes@[j].priority) < priority_number(me.priority)
                || (w.nodes@[j].priority == me.priority
                    && w.nodes@[j].ticket.unwrap() < me.ticket.unwrap())) by {
            assert(!pool_precedes(w.nodes@[j], me));
        }
    } else {
        let j = choose|j: int| 0 <= j < w.nodes@.len() && pool_precedes(w.nodes@[j], me);
        assert(!(w.nodes@[j].phase is Ready) && w.nodes@[j].ticket.is_some());
    }
}
}
