//! Q48 / #806: leaf bypass, no-steal and oldest-waiter scheduler lemmas.
use super::*;

verus! {
/// Q48: a real grant to a younger leaf excludes every older overlapping waiter.
pub(super) proof fn bypass_safety(before: World, after: World, i: int, older: int)
    requires 0 <= i < before.nodes@.len(), 0 <= older < before.nodes@.len(),
        before.nodes@[i].phase == Phase::Ready(Resume::Making), !before.nodes@[i].paused,
        before.nodes@[i].ticket.is_some(), !before.nodes@[i].holds,
        before.nodes@[older].phase is Ready, !before.nodes@[older].paused,
        retirement_gate::older(before, i, older),
        after.nodes@[i] == retirement_gate::slot_node(before.nodes@[i], before.next_ticket,
            retirement_gate::slot(before, i) && retirement_gate::line(before, i)),
    ensures after.nodes@[i].holds ==> footprint::disjoint(before,
        before.nodes@[older].footprint, before.nodes@[i].footprint),
{
    reveal(retirement_gate::slot_node);
    if after.nodes@[i].holds {
        assert(retirement_gate::eligible(before, i, older));
    }
}

/// Q48: once the oldest waiter can run, every younger ticket stays in place.
pub(super) proof fn no_steal(before: World, i: int, oldest: int)
    requires 0 <= i < before.nodes@.len(), 0 <= oldest < before.nodes@.len(),
        retirement_gate::older(before, i, oldest),
        retirement_gate::slot(before, oldest), retirement_gate::line(before, oldest),
    ensures !retirement_gate::line(before, i),
{
    if retirement_gate::line(before, i) {
        assert(retirement_gate::eligible(before, i, oldest));
        retirement_gate::line_excludes_overlap(before, oldest, before.nodes@[oldest].ticket.unwrap());
    }
}

/// Q48: no earlier Ready ticket means the real line scan accepts this waiter.
pub(super) proof fn oldest_line(w: World, i: int)
    requires 0 <= i < w.nodes@.len(), w.nodes@[i].ticket.is_some(),
        forall|j: int| 0 <= j < w.nodes@.len() ==> !retirement_gate::older(w, i, j),
    ensures retirement_gate::line(w, i),
{
}
}
