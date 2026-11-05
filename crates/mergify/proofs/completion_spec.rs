use vstd::prelude::*;
use crate::completion::*;

verus! {
/// #608: the previous cycle is stale at the equality boundary and throughout u64.
/// No sequence bound; this theorem applies to every event and saved cycle id.
fn stale_cycle(event: u64, saved: u64)
    requires event <= saved,
{
    assert(!fresh(event, Freshness::Intent { last_event: Some(saved) }));
}

/// #373/#608: concrete witnesses exercise confirmed-conflict and receipt paths.
fn completion_witness() {
    let conflict = resolve::<u8>(Complete::Confirmed { progress: false, conflict: true }, true);
    assert(matches!(conflict, Resolution::Found { ack: Ack::Confirmed, conflict: true, progress: false }));
    assert(confirmed(Receipt::Found));
    assert(!confirmed(Receipt::Absent));
    assert(conflicts(Some(false)));
    assert(!conflicts(None));
}

/// #608: a merged queue skips the missing hold read; a saved baseline skips repaired_head.
fn hold_witness() {
    assert(held(Queue::Merged, None) == Some(false));
    assert(held(Queue::Other, None) == None);
    assert(person_events(true, None) == Some(true));
    assert(person_events(false, None) == None);
    assert(fresh(0, Freshness::Snapshot { unresolved: true }));
    assert(hold_candidate(HoldEvidence::Person));
    assert(hold_candidate(HoldEvidence::Command));
    assert(!hold_candidate(HoldEvidence::Other));
    assert(unresolved_hold(None));
    assert(unresolved_hold(Some((9, 9))));
    assert(!unresolved_hold(Some((9, 10))));
    assert(!fresh(9, Freshness::Intent { last_event: Some(9) }));
    assert(fresh(10, Freshness::Intent { last_event: Some(9) }));
}

/// #608: max id, last on a tie, and absence each have a concrete witness.
fn newest_witness() {
    let at = time(-1, 1_999_999_999);
    let events = vec![
        LabelEvent { id: 2, label: Label::Dequeued, action: LabelAction::Labeled, at },
        LabelEvent { id: 3, label: Label::Queued, action: LabelAction::Labeled, at },
        LabelEvent { id: 2, label: Label::Dequeued, action: LabelAction::Other, at },
    ];
    let chosen = newest(&events, Label::Dequeued);
    assert(chosen == Some(2));
    assert(newest(&events, Label::Queue) == None);
    assert(has_label(&vec![Label::Queue], Label::Queue));
    assert(!labeled(LabelAction::Other));
    assert(dequeue_gate(true, false, false) == DequeueGate::Pending);
    assert(dequeue_gate(true, false, true) == DequeueGate::Events);
}

/// #760: all ten field equalities can hold; one changed field acknowledges change.
fn check_witness() {
    assert(!check_changed(CheckComparison::BothMissing));
    assert(check_changed(CheckComparison::OneMissing));
    assert(!check_changed(CheckComparison::BothPresent { fields: [true, true, true, true, true, true, true, true, true, true] }));
    assert(check_changed(CheckComparison::BothPresent { fields: [false, false, false, false, false, false, false, false, false, false] }));
}
}
