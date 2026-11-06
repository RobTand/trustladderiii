//! Dispatch permission is volatile, bound to identity, and consumed by value.
#[path = "../src/step.rs"] mod delivery;
use delivery::*;
use vstd::prelude::*;
verus! {
#[verifier::opaque]
spec fn no_fresh_issue(out: Action) -> bool {
    !matches!(out, Action::CommitSent { .. } | Action::Execute(_) | Action::ReadHistory(_))
}

proof fn start_has_no_fresh_issue(v: DeliveryView, out: Action)
    requires matches!(v.state, State::Sent | State::Uncertain | State::Cancelling), start_contract(v, out),
    ensures no_fresh_issue(out),
{
    reveal(no_fresh_issue);
}

fn no_fresh_dispatch_from_active(v: DeliveryView)
    requires matches!(v.state, State::Sent | State::Uncertain | State::Cancelling),
{
    let next = action(v, Input::Start);
    proof { start_has_no_fresh_issue(v, next); }
    assert(no_fresh_issue(next));
}

fn execute_requires_sent_observation(v: DeliveryView, input: Input) {
    let next = action(v, input);
    if let Action::Execute(p) = next {
        assert(v.state == State::Sent);
        assert(permission_at(p, v));
        assert(matches!(input, Input::Observed { revision_changed: false, .. }));
    }
}

fn revision_change_discards_permission(v: DeliveryView, pending: Continuation) {
    let next = action(v, Input::Observed { pending, revision_changed: true });
    assert(!matches!(next, Action::Execute(_)));
}

fn changed_binding_discards_permission(v: DeliveryView, old: DeliveryView, pending: Continuation)
    requires permission_at(pending, old),
        v.key@ != old.key@ || v.epoch != old.epoch || v.revision != old.revision,
{
    let next = action(v, Input::Observed { pending, revision_changed: false });
    assert(matches!(next, Action::Wait));
}

fn no_permission_before_sent(v: DeliveryView)
    requires v.state == State::Intended, v.fence == Fence::Live, !v.waits.outage,
        v.approval != (ApprovalGate::Prior { predecessor: false }),
        v.approval != (ApprovalGate::Prior { predecessor: true }),
{
    let next = action(v, Input::Start);
    assert(matches!(next, Action::CommitSent { .. }));
    assert(!matches!(next, Action::Execute(_)));
}

fn executed_ack_survives_revision_change(v: DeliveryView, pending: Continuation, ack: Ack)
    requires permission_key(pending) == v.key@, permission_epoch(pending) == v.epoch,
        permission_stage(pending) == Stage::Execute, v.state == State::Sent,
{
    let next = action(v, Input::Executed { pending, outcome: Outcome::Acknowledged(ack) });
    assert(matches!(next, Action::EnqueueCompletion(a) if a == ack));
}

}
fn main() {}
