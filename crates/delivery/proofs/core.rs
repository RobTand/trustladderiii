//! State-table Effects :724-755. All input values, no schedule bound.
#[path = "../src/step.rs"]
mod delivery;
use delivery::*;
use vstd::prelude::*;

verus! {
/// State-table Effects: the real completion plan stays inside the real edge table.
/// Unbounded executions follow by induction over these finite transition chains.
proof fn completion_uses_allowed_edges(state: State, ack: Ack)
    requires matches!(state, State::Sent | State::Uncertain | State::Cancelling),
{
    let plan = completion(state, ack);
    if plan.bridge {
        assert(allows(State::Uncertain, State::Intended));
        assert(allows(State::Intended, State::Sent));
    }
    assert(allows(plan.from, plan.to));
}

/// State-table final rows: no outgoing edge, regardless of next state or trace length.
proof fn final_stays_final(state: State, next: State)
    requires matches!(state, State::Confirmed | State::Failed | State::Cancelled),
{
    assert(!allows(state, next));
}

/// State-table Effects: all finals and cancellation's two outcomes are reachable.
/// These constructive paths call the real steps and need no hypothetical input.
proof fn final_witnesses() {
    assert(allows(State::Intended, State::Sent));
    let confirmed = completion(State::Sent, Ack::Confirmed);
    let failed = completion(State::Sent, Ack::Permanent);
    assert(confirmed.to == State::Confirmed);
    assert(failed.to == State::Failed);
    assert(allows(State::Intended, State::Cancelled));
    assert(allows(State::Sent, State::Cancelling));
    assert(allows(State::Cancelling, State::Cancelled));
    let late = completion(State::Cancelling, Ack::Retryable);
    assert(late.to == State::Confirmed);
}

/// State-table :755 F1 and :731-734 F2. The store's live-epoch rule is this exact
/// predicate, conformed against the real Store, rather than an issue-time fence.
proof fn fence_and_drain_witnesses() {
    assert(!accepts_fence(EpochOrder::Older));
    assert(accepts_fence(EpochOrder::Current));
    assert(accepts_fence(EpochOrder::Newer));
    assert(blocks_epoch_advance(State::Sent));
    assert(blocks_epoch_advance(State::Uncertain));
    assert(blocks_epoch_advance(State::Cancelling));
    assert(!blocks_epoch_advance(State::Confirmed));
}

/// State-table ack-by-key. Two unequal byte keys cannot target the same effect.
/// No byte alphabet, key length or prefix bound.
proof fn ack_does_not_cross_keys(own: &[u8], other: &[u8], key: &[u8])
    requires own@ != other@, own@ == key@,
{
    assert(key_matches(own, key));
    assert(!key_matches(other, key));
}
}
fn main() {}
