use crate::enqueue::*;
use crate::enqueue_helpers::*;
use crate::Head;
use vstd::prelude::*;

verus! {
pub open spec fn command(old: EnqueueState, label: bool) -> bool {
    match old.intent { None => label, Some(i) => i.requeue.is_some() }
}

/// Publishing/Queued, docs/state-table.md:537-546,573-580; Q41/Q47/Q52;
/// #608/#751/#760/#2705: saves precede final validation and every upstream write.
/// Quantification ranges over the real non-Kani Vec, without a slot bound.
pub open spec fn contract(old: EnqueueState, event: QueueEvent, next: EnqueueState, effects: Effects) -> bool {
    (!refused(old) ==> match event {
        QueueEvent::Prepare { dequeue_pending: false, checks_ready: true, queue_label } => {
            effects@ == Seq::empty().push(Effect::SaveIntent).push(Effect::Read(if command(old, queue_label) { Need::Command } else { Need::LabelPull }))
            && next.intent == match old.intent {
                Some(i) => Some(i),
                None => Some(Intent { requeue: if queue_label { Some(Requeue::Ready) } else { None } }),
            }
        },
        QueueEvent::LabelRead { head, open, merged } => {
            next.intent == old.intent && effects@ == seq![
                if old.intent != Some(Intent { requeue: None }) { Effect::Error(Failure::IntentMissing) }
                else if head != Head::Current { Effect::Complete(Complete::Retryable) }
                else if !open || merged { Effect::Error(Failure::PullClosed) }
                else { Effect::Write(Write::Label) }
            ]
        },
        _ => true,
    }) && forall|i: int| 0 <= i < effects@.len() && #[trigger] effects@[i] == Effect::Write(Write::Label) ==> {
        matches!(event, QueueEvent::LabelRead { .. })
        && old.intent == Some(Intent { requeue: None }) && next.intent.is_some()
    }
}
}
