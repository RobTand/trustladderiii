use vstd::prelude::*;
use crate::completion::*;

verus! {
/// #608: the previous cycle is stale at the equality boundary and throughout u64.
/// No sequence bound; this theorem applies to every event and saved cycle id.
fn stale_cycle(event: u64, saved: u64)
    requires event <= saved,
{
    let actual_0 = fresh(event, Freshness::Intent { last_event: Some(saved) });
    assert(!actual_0);
}

/// #373/#608: concrete witnesses exercise confirmed-conflict and receipt paths.
fn completion_witness() {
    let conflict = resolve::<u8>(Complete::Confirmed { progress: false, conflict: true }, true);
    assert(matches!(conflict, Resolution::Found { ack: Ack::Confirmed, conflict: true, progress: false }));
    let actual_1 = confirmed(Receipt::Found);
    assert(actual_1);
    let actual_2 = confirmed(Receipt::Absent);
    assert(!actual_2);
    let actual_3 = conflicts(Some(false));
    assert(actual_3);
    let actual_4 = conflicts(None);
    assert(!actual_4);
}

/// #608: a merged queue skips the missing hold read; a saved baseline skips repaired_head.
fn hold_witness() {
    let actual_5 = held(Queue::Merged, None);
    assert(actual_5 == Some(false));
    let actual_6 = held(Queue::Other, None);
    assert(actual_6 == None);
    let actual_7 = person_events(true, None);
    assert(actual_7 == Some(true));
    let actual_8 = person_events(false, None);
    assert(actual_8 == None);
    let actual_9 = fresh(0, Freshness::Snapshot { unresolved: true });
    assert(actual_9);
    let actual_10 = hold_candidate(HoldEvidence::Person);
    assert(actual_10);
    let actual_11 = hold_candidate(HoldEvidence::Command);
    assert(actual_11);
    let actual_12 = hold_candidate(HoldEvidence::Other);
    assert(!actual_12);
    let actual_13 = unresolved_hold(None);
    assert(actual_13);
    let actual_14 = unresolved_hold(Some((9, 9)));
    assert(actual_14);
    let actual_15 = unresolved_hold(Some((9, 10)));
    assert(!actual_15);
    let actual_16 = fresh(9, Freshness::Intent { last_event: Some(9) });
    assert(!actual_16);
    let actual_17 = fresh(10, Freshness::Intent { last_event: Some(9) });
    assert(actual_17);
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
    let actual_18 = newest(&events, Label::Queue);
    assert(actual_18 == None);
    let actual_19 = has_label(&vec![Label::Queue], Label::Queue);
    assert(actual_19);
    let actual_20 = labeled(LabelAction::Other);
    assert(!actual_20);
    let actual_21 = dequeue_gate(true, false, false);
    assert(actual_21 == DequeueGate::Pending);
    let actual_22 = dequeue_gate(true, false, true);
    assert(actual_22 == DequeueGate::Events);
}

/// #760: all ten field equalities can hold; one changed field acknowledges change.
fn check_witness() {
    let actual_23 = check_changed(CheckComparison::BothMissing);
    assert(!actual_23);
    let actual_24 = check_changed(CheckComparison::OneMissing);
    assert(actual_24);
    let actual_25 = check_changed(CheckComparison::BothPresent { fields: [true, true, true, true, true, true, true, true, true, true] });
    assert(!actual_25);
    let bad = [false, false, false, false, false, false, false, false, false, false];
    let actual_26 = check_changed(CheckComparison::BothPresent { fields: bad });
    assert(!bad@[0]);
    assert(actual_26);
}
}
