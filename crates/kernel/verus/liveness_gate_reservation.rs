//! G22 spike: reserve a newly available slot for a runnable older waiter.
use super::*;

verus! {
pub(super) open spec fn runnable_older(w: World, older: int, younger: int) -> bool {
    0 <= older < w.nodes@.len() && 0 <= younger < w.nodes@.len()
        && w.nodes@[older].phase is Ready && w.nodes@[younger].phase is Ready
        && w.nodes@[older].repo == w.nodes@[younger].repo
        && w.nodes@[older].ticket.is_some() && w.nodes@[younger].ticket.is_some()
        && w.nodes@[older].ticket.unwrap() > w.nodes@[younger].ticket.unwrap()
        && scheduler::slot_spec(w, older)
        && !scheduler::overlapped_by_older(w, older, w.nodes@[older].ticket.unwrap())
}

pub(super) proof fn older_disqualifies_younger(w: World, older: int, younger: int)
    requires runnable_older(w, older, younger),
    ensures !oldest_waiter(w, younger),
{
    reveal(oldest_waiter);
    assert(w.nodes@[older].ticket.unwrap() < w.nodes@[younger].ticket.unwrap());
}
}

kernel_fn! {
pub(super) fn younger_cannot_take_line(w: &World, older: usize, younger: usize)
    -> (result: bool) [
    requires scheduling_safe(*w), runnable_older(*w, older as int, younger as int),
    ensures !result,
] {
    let first = first_in_line(w, younger);
    first
}
}
