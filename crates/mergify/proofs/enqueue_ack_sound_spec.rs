use crate::enqueue::*;
use crate::enqueue_helpers::*;
use crate::{Head, QueueState};
use vstd::prelude::*;

verus! {
/// Independent ordered oracle ported from enqueue_ack_sound, including its
/// pre-enqueue conflict completion and independent later recovery sample.
pub open spec fn expected(old: EnqueueState, read: QueueRead) -> Effect {
    if refused(old) { Effect::SaveRecovery }
    else if read.queue != QueueState::Merged && read.held { Effect::Complete(Complete::Pending) }
    else if !old.confirmed && read.queue != QueueState::Merged
        && old.intent == Some(Intent { requeue: Some(Requeue::Sending) }) && read.refusal.is_none() {
        Effect::Read(Need::Refusal)
    } else if !old.confirmed && read.queue != QueueState::Merged
        && old.intent == Some(Intent { requeue: Some(Requeue::Sending) }) && read.refusal == Some(true) {
        Effect::SaveRecovery
    } else if !old.confirmed && read.reruns_pending.is_none() { Effect::Read(Need::Reruns) }
    else if !old.confirmed && read.reruns_pending == Some(true) { Effect::Complete(Complete::Pending) }
    else if read.head == Head::Other && read.queue != QueueState::Merged && !old.confirmed {
        Effect::Complete(Complete::Retryable)
    } else if read.head == Head::Current && !read.merged && read.conflicts {
        Effect::Complete(Complete::Confirmed { progress: false, conflict: true })
    } else if read.person_removed.is_none() { Effect::Read(Need::Person) }
    else if old.confirmed || read.person_removed == Some(true) || read.queue == QueueState::Merged
        || read.head == Head::Current && (matches!(read.queue, QueueState::Queued | QueueState::ChecksRunning)
            || matches!(read.queue, QueueState::Dequeued | QueueState::FailedCi | QueueState::Conflict | QueueState::DequeuedByPerson)
                && old.intent == Some(Intent { requeue: None }) && read.check_changed) {
        Effect::Complete(Complete::Confirmed { progress: true, conflict: false })
    } else { match read.issue { None => Effect::Read(Need::Issue),
        Some(issue) => outcome(old.intent, read.queue, issue) } }
}

/// #751 K-ack-sound; docs/state-table.md:526-535 Publishing confirms step 4
/// before Queued (:570). Current-head authority: Q41, state-machine.md:132;
/// #608 requires fresh exact-head checks and retained refusal handling.
/// #760 preserves separate recovery samples. The exact oracle, conflict
/// completion and receipt encoding are preserved existing behaviour (no rule).
/// Full typed domains and real Vec effects; no temporal or slot bound.
pub open spec fn contract(old: EnqueueState, event: QueueEvent, next: EnqueueState, effects: Effects) -> bool {
    match event { QueueEvent::Reconcile(read) => {
        let first = expected(old, read);
        effects@.len() > 0 && effects@[0] == first
        && next.confirmed == (old.confirmed || matches!(first, Effect::Complete(Complete::Confirmed { .. })))
        && if refused(old) {
            effects@.len() == 2 && effects@[1] == Effect::Complete(Complete::Permanent(Failure::Refused))
        } else if first == Effect::SaveRecovery {
            effects@.len() == 3 && effects@[1] == Effect::SaveIntent
            && effects@[2] == Effect::Complete(Complete::Permanent(Failure::Refused))
            && next.intent == Some(Intent { requeue: Some(Requeue::Refused) })
        } else { effects@.len() == 1 }
    }, _ => true }
}
}
