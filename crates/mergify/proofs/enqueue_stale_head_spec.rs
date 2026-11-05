use crate::enqueue::*;
use crate::enqueue_helpers::*;
use crate::{Head, QueueState};
use vstd::prelude::*;

verus! {
pub open spec fn stale(old: EnqueueState, event: QueueEvent) -> bool {
    match event {
        QueueEvent::Reconcile(read) => read.head == Head::Other && read.queue != QueueState::Merged && !old.confirmed,
        QueueEvent::LabelRead { head, .. } | QueueEvent::RerunRead { head, .. } => head == Head::Other,
        QueueEvent::DispatchCommand(read) => read.head == Head::Other,
        _ => false,
    }
}

/// #751 K-stale-head and #608 fresh head validation before mutation.
/// Q41 state-machine.md:132 binds approval to the exact commit;
/// state-table.md:667 requires a changed Publishing/Queued head to be retested.
/// Publishing/Queued context: state-table.md:526,570. #760 preserves gates.
/// Prior refusal and pending-read precedence are preserved existing behaviour
/// (no rule): stale inputs cannot erase a terminal refusal.
/// Full event/state domains; quantifies the actual Vec without a slot bound.
pub open spec fn contract(old: EnqueueState, event: QueueEvent, next: EnqueueState, effects: Effects) -> bool {
    (stale(old, event) ==> forall|i: int| 0 <= i < effects@.len() ==>
        match #[trigger] effects@[i] { Effect::Write(_) | Effect::Complete(Complete::Confirmed { .. }) => false, _ => true })
    && match event { QueueEvent::Reconcile(read) => {
        (!refused(old) && stale(old, event) && read.held ==> effects@ == seq![Effect::Complete(Complete::Pending)])
        && ((stale(old, event) && !refused(old) && !read.held && read.reruns_pending == Some(false)
            && (old.intent != Some(Intent { requeue: Some(Requeue::Sending) }) || read.refusal == Some(false)))
        ==> effects@.len() > 0 && effects@[0] == Effect::Complete(Complete::Retryable))
        },
        _ => true,
    }
}
}
