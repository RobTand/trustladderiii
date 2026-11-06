//! Reconciliation and completion have distinct durable boundaries.
#[path = "../src/step.rs"] mod delivery;
use delivery::*;
#[path = "../src/upstream.rs"] mod upstream;
use upstream::*;
use vstd::prelude::*;
verus! {
#[verifier::opaque]
spec fn enqueued_only(out: Action) -> bool {
    matches!(out, Action::EnqueueCompletion(_))
}

fn found_enqueues_without_committing(v: DeliveryView, pending: Continuation, ack: Ack)
    requires permission_at(pending, v),
{
    let next = action(v, Input::Reconciled { pending, evidence: Evidence::Found(ack) });
    assert(!matches!(next, Action::Commit(_) | Action::Transition(_)));
}

fn pending_never_proves_absence(v: DeliveryView, pending: Continuation) {
    let next = action(v, Input::Reconciled { pending, evidence: Evidence::Pending });
    assert(!matches!(next, Action::Transition(State::Intended) | Action::CancelAbsent | Action::EnqueueCompletion(_)));
}

fn read_error_is_not_acknowledgement(v: DeliveryView, pending: Continuation) {
    let next = action(v, Input::Reconciled { pending, evidence: Evidence::Error });
    assert(matches!(next, Action::Wait));
}

fn consumption_retires_old_retry_key(v: DeliveryView, successor: bool)
    requires v.kind == DeliveryKind::KernelEffect, matches!(v.state, State::Sent | State::Uncertain),
{
    let next = action(v, Input::Consume { key: v.key, ack: Ack::Retryable, kernel_ack: Ack::Retryable, approval_successor: successor });
    assert(matches!(next, Action::Commit(p) if p.retire_retry && p.remove_watch && p.completion.to == State::Intended));
}

/// Quiescent reads have no schedule bound. A held absence preserves U; a
/// Found queues evidence, and only a later Consume changes the effect state.
fn reconciliation_agrees_with_quiescent_upstream(v: DeliveryView, before: Upstream)
    requires v.state == State::Uncertain, v.fence == Fence::Live,
        !v.waits.outage, !v.waits.quota, before.outstanding == 0, before.incorrect <= before.copies,
{
    let sampled = read(before, Class::Converges);
    let first = action(v, Input::Start);
    if let Action::Observe(pending) = first {
        let second = action(v, Input::Observed { pending, revision_changed: false });
        if let Action::Reconcile(pending) = second {
            let evidence = if sampled.found { Evidence::Found(Ack::Confirmed) } else { Evidence::Absent };
            let next = action(v, Input::Reconciled { pending, evidence });
            reveal(enqueued_only);
            assert(enqueued_only(next) == (before.copies > 0));
            if !sampled.found && v.waits.held { assert(matches!(next, Action::Wait)); }
        } else { assert(false); }
    } else { assert(false); }
}

/// Preserve the missing closed-node recovery witness. Finals are excluded
/// from pending delivery, even when an old upstream request lands after restart.
fn late_landing_after_final_exposes_missing_recovery(v: DeliveryView)
    requires v.state == State::Cancelled,
{
    let old = issue(Upstream { copies: 0, incorrect: 0, outstanding: 0, repair_debt: 0, found: false });
    let absent = read(lose_response(old), Class::Converges);
    assert(!absent.found);
    let next = action(v, Input::Start); assert(matches!(next, Action::Wait));
    let zombie = land(restart(absent), Class::Converges, true,
        OnceLaws { atomic_original_key_dedup: false, stable_operation_identity: false });
    assert(zombie.copies == 1 && zombie.outstanding == 0);
    let next = action(v, Input::Start); assert(matches!(next, Action::Wait));
}
}
fn main() {}
