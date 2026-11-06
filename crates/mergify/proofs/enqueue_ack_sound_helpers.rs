//! A4-owned exact result contract for the actual reconcile_issue helper.
use crate::enqueue::*;
use crate::QueueState;
pub use crate::enqueue_shared::{completion, refused};
use vstd::prelude::*;

verus! {
pub open spec fn pending(value: Option<bool>, need: Need) -> Effect {
    match value { None => Effect::Read(need), Some(true) => Effect::Complete(Complete::Pending),
        Some(false) => Effect::Complete(Complete::Absent) }
}

/// #751 K-ack-sound and #760's independent recovery-file samples.
/// Exact read/outcome encoding: preserved existing behaviour (no rule).
pub open spec fn outcome(intent: Option<Intent>, queue: QueueState, read: IssueRead) -> Effect {
    let waiting = queue == QueueState::Waiting && read.queue_label;
    if waiting && read.recovery.is_none() { Effect::Read(Need::RecoveryFile) }
    else if waiting && read.recovery == Some(true) { Effect::Complete(Complete::Pending) }
    else if waiting && read.repaired.is_none() { Effect::Read(Need::RepairedHead) }
    else if waiting && read.repaired == Some(true) { Effect::Complete(Complete::Pending) }
    else { match intent {
        None => pending(read.dequeue_pending, Need::Dequeue),
        Some(Intent { requeue: Some(command) }) => match read.requeue_recovery {
            None => Effect::Read(Need::RequeueRecoveryFile),
            Some(true) => Effect::Complete(Complete::Pending),
            Some(false) => if command == Requeue::Sending { Effect::Complete(Complete::Pending) }
                else { pending(read.dequeue_pending, Need::Dequeue) },
        },
        Some(Intent { requeue: None }) => if read.queue_label { Effect::Complete(Complete::Pending) }
            else { pending(read.applied, Need::Applied) },
    } }
}
pub open spec fn issue(intent: Option<Intent>, queue: QueueState, read: IssueRead, result: Effect) -> bool {
    result == outcome(intent, queue, read)
}
}
