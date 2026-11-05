use crate::enqueue::*;
use crate::enqueue_helpers::*;
use crate::Head;
use vstd::prelude::*;

verus! {
pub open spec fn command(old: EnqueueState, label: bool) -> bool {
    match old.intent { None => label, Some(i) => i.requeue.is_some() }
}

/// TL3 #751 K-write-ahead requires recorded intent before every upstream write;
/// #760 preserves SaveIntent/fence/final-read order. Q41 exact-head validation:
/// docs/state-machine.md:132,487. Recovery context: docs/state-table.md:541,575.
/// #608 records served incident RobTand/prismabuild#2705, not a new rule.
/// Exact effect order and intent encoding: preserved existing behaviour (no rule).
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
