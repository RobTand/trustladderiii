//! F1-F3: entry fencing is separate from permission already admitted by an adapter.
#[path = "../src/step.rs"] mod delivery;
use delivery::*;
use vstd::prelude::*;
verus! {
fn entry_refuses_older_and_accepts_equal(order: EpochOrder) {
    let accepted = accepts_fence(order);
    assert(accepted == (order != EpochOrder::Older));
    let advance = advances_fence(order);
    assert(advance == (order == EpochOrder::Newer));
}

fn active_deliveries_block_advance(state: State)
    requires matches!(state, State::Sent | State::Uncertain | State::Cancelling),
{
    let blocked = blocks_epoch_advance(state); assert(blocked);
}

#[verifier::opaque]
spec fn zombie_allowed(entry_admitted: bool, durably_sent: bool, owns_now: bool) -> bool {
    entry_admitted && durably_sent
}

/// An entry accepted at equality can issue after losing ownership. This is an
/// environment relation, not a new issue-time check or an exactly-once promise.
fn zombie_issue_witness() {
    let admitted = accepts_fence(EpochOrder::Current);
    let blocked = blocks_epoch_advance(State::Sent);
    reveal(zombie_allowed);
    assert(zombie_allowed(admitted, blocked, false));
    let retired = blocks_epoch_advance(State::Cancelled); assert(!retired);
    assert(zombie_allowed(admitted, blocked, false));
}
}
fn main() {}
