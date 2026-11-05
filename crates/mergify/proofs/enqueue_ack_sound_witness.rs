use crate::enqueue::*;
use crate::{Head, QueueState};
use vstd::prelude::*;

verus! {
/// Original covers: recovery appears between reads; await acknowledgement.
fn recovery_appears_between_reads() {
    let state = EnqueueState { intent: Some(Intent { requeue: Some(Requeue::Ready) }),
        confirmed: false, recovery: None, rerun: None };
    let issue = IssueRead { queue_label: true, recovery: Some(false), repaired: Some(false),
        requeue_recovery: Some(true), dequeue_pending: Some(false), applied: Some(false) };
    let read = QueueRead { head: Head::Current, queue: QueueState::Waiting, merged: false,
        conflicts: false, held: false, check_changed: false, refusal: Some(false),
        reruns_pending: Some(false), person_removed: Some(false), issue: Some(issue) };
    let (_, effects) = decide(state, QueueEvent::Reconcile(read));
    assert(issue.recovery == Some(false) && issue.repaired == Some(false) && issue.requeue_recovery == Some(true));
    assert(effects@ == seq![Effect::Complete(Complete::Pending)]);
}

/// Original cover: no write was applied.
fn absent() {
    let state = EnqueueState { intent: Some(Intent { requeue: None }), confirmed: false, recovery: None, rerun: None };
    let read = QueueRead { head: Head::Current, queue: QueueState::Unknown, merged: false,
        conflicts: false, held: false, check_changed: false, refusal: None, reruns_pending: Some(false),
        person_removed: Some(false), issue: Some(IssueRead { queue_label: false, recovery: None,
            repaired: None, requeue_recovery: None, dequeue_pending: None, applied: Some(false) }) };
    let (_, effects) = decide(state, QueueEvent::Reconcile(read));
    assert(effects@ == seq![Effect::Complete(Complete::Absent)]);
}

/// Original cover: changed terminal acknowledges without a command requeue.
fn changed_terminal() {
    let state = EnqueueState { intent: Some(Intent { requeue: None }), confirmed: false, recovery: None, rerun: None };
    let read = QueueRead { head: Head::Current, queue: QueueState::FailedCi, merged: false,
        conflicts: false, held: false, check_changed: true, refusal: None, reruns_pending: Some(false),
        person_removed: Some(false), issue: None };
    let (next, effects) = decide(state, QueueEvent::Reconcile(read));
    assert(!state.confirmed && read.queue == QueueState::FailedCi);
    assert(effects@ == seq![Effect::Complete(Complete::Confirmed { progress: true, conflict: false })]);
    assert(next.confirmed);
}

/// Original cover: pre-enqueue conflict is an acknowledged conflict outcome.
fn conflict() {
    let state = EnqueueState { intent: None, confirmed: false, recovery: None, rerun: None };
    let read = QueueRead { head: Head::Current, queue: QueueState::Conflict, merged: false,
        conflicts: true, held: false, check_changed: false, refusal: None, reruns_pending: Some(false),
        person_removed: None, issue: None };
    let (next, effects) = decide(state, QueueEvent::Reconcile(read));
    assert(effects@ == seq![Effect::Complete(Complete::Confirmed { progress: false, conflict: true })]);
    assert(next.confirmed);
}
}
