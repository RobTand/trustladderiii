use crate::execute::*;
use vstd::prelude::*;

verus! {
/// Effects / H2: initial confirmation needs a receipt read; final confirmation saves.
/// Five concrete witnesses; no sequence bound is imposed on the production function.
fn receipts_are_reachable() {
    let first = receipt(ReceiptStage::Initial, Receipt::Confirmed, None);
    assert(first == ReceiptAction::ReadRecord);
    let missing = receipt(ReceiptStage::Initial, Receipt::Confirmed, Some(false));
    assert(missing == ReceiptAction::SaveRecord);
    let present = receipt(ReceiptStage::Initial, Receipt::Confirmed, Some(true));
    assert(present == ReceiptAction::Return);
    let last = receipt(ReceiptStage::Final, Receipt::Confirmed, None);
    assert(last == ReceiptAction::SaveRecord);
    let absent = receipt(ReceiptStage::Initial, Receipt::Absent, None);
    assert(absent == ReceiptAction::Prepare);
}

/// Publishing step 4 / K-ack-sound: admission, Conflict and stale-head outcomes exist.
/// Four concrete witnesses; the theorem covers the complete finite input domain.
fn acknowledgements_are_reachable() {
    let active = acknowledgement(AckRead {
        current: true, merged: false, conflicts: false, confirmed: false,
        person_removed: Some(false), queue: AdmissionQueue::Active,
        no_requeue: false, check_changed: false,
    });
    assert(active == AckDecision::Confirmed);
    let stale = acknowledgement(AckRead {
        current: false, merged: false, conflicts: false, confirmed: false,
        person_removed: Some(true), queue: AdmissionQueue::Active,
        no_requeue: false, check_changed: false,
    });
    assert(stale == AckDecision::Retryable);
    let conflict = acknowledgement(AckRead {
        current: true, merged: false, conflicts: true, confirmed: false,
        person_removed: None, queue: AdmissionQueue::Other,
        no_requeue: false, check_changed: false,
    });
    assert(conflict == AckDecision::Conflict);
    let absent = acknowledgement(AckRead {
        current: true, merged: false, conflicts: false, confirmed: false,
        person_removed: Some(false), queue: AdmissionQueue::Other,
        no_requeue: false, check_changed: false,
    });
    assert(absent == AckDecision::ReadIssue);
}

/// Effects / #751: both durability boundaries precede dispatch eligibility.
/// Exactly two ordered boundaries; all label eligibility inputs are unbounded Booleans.
fn write_ahead_is_reachable() {
    let boundaries = intent_boundaries();
    assert(boundaries[0] == IntentBoundary::Save);
    assert(boundaries[1] == IntentBoundary::Fence);
    let write = label(true, true, true, false);
    assert(write == LabelDecision::Write);
    let stale = label(true, false, true, false);
    assert(stale == LabelDecision::Retryable);
    let missing = label(false, true, true, false);
    assert(missing == LabelDecision::IntentMissing);
}

/// H6 / #608: strict event ID freshness, timestamp equality and nanoseconds matter.
/// Six concrete witnesses; u64 IDs, i64 seconds and u32 nanos span their full domains.
fn dequeue_freshness_is_reachable() {
    let probe = dequeue_probe(true, false, true);
    assert(probe == DequeueProbe::Events);
    let fresh = dequeue_fresh(true, 2, Some(1), (0, 0), None);
    assert(fresh);
    let stale = dequeue_fresh(true, 1, Some(1), (1, 0), Some((0, 0)));
    assert(!stale);
    let simultaneous = dequeue_fresh(true, 0, None, (-1, 3), Some((-1, 3)));
    assert(simultaneous);
    let nanos = dequeue_fresh(true, 0, None, (-1, 2), Some((-1, 3)));
    assert(!nanos);
    let unlabeled = dequeue_fresh(false, 2, Some(1), (0, 0), None);
    assert(!unlabeled);
}

/// Human hold rows / H1-H2: a saved intent preserves its event cursor.
/// Five witnesses; latest_event is an induction step over an arbitrary event history.
fn person_and_baseline_are_reachable() {
    let baseline = person_probe(true, None);
    assert(baseline == PersonProbe::Events);
    let repaired = person_probe(false, None);
    assert(repaired == PersonProbe::RepairedHead);
    let fresh = person_fresh(Some(1), 2);
    assert(fresh);
    let stale = person_fresh(Some(2), 2);
    assert(!stale);
    let label = baseline_intent(false);
    assert(label == BaselineIntent::Label);
    let requeue = baseline_intent(true);
    assert(requeue == BaselineIntent::Requeue);
    let first = latest_event(0, 7);
    let next = latest_event(first, 3);
    assert(next == 7);
}
}
