use crate::enqueue::*;
use crate::enqueue_contract::command;
use crate::enqueue_helpers::*;
use crate::Head;
use vstd::prelude::*;

verus! {
/// Composes Prepare and final LabelRead over all original arbitrary inputs.
fn final_validation(old: EnqueueState, label: bool, head: Head, open: bool, merged: bool)
    requires !refused(old), !command(old, label),
{
    let (prepared, effects) = decide(old, QueueEvent::Prepare {
        dequeue_pending: false, checks_ready: true, queue_label: label });
    assert(effects@ == Seq::empty().push(Effect::SaveIntent).push(Effect::Read(Need::LabelPull)));
    let (validated, final_effects) = decide(prepared, QueueEvent::LabelRead { head, open, merged });
    assert(final_effects@ == seq![if head == Head::Other { Effect::Complete(Complete::Retryable) }
        else if !open || merged { Effect::Error(Failure::PullClosed) }
        else { Effect::Write(Write::Label) }]);
    assert(validated.intent == Some(Intent { requeue: None }));
}

/// Three enqueue_write_ahead covers: composed stale/closed reads and a label write.
fn covers() {
    let state = EnqueueState { intent: None, confirmed: false, recovery: None, rerun: None };
    let (prepared, saved) = decide(state, QueueEvent::Prepare {
        dequeue_pending: false, checks_ready: true, queue_label: false });
    assert(saved@[0] == Effect::SaveIntent);
    let (_, stale) = decide(prepared, QueueEvent::LabelRead { head: Head::Other, open: true, merged: false });
    assert(stale@ == seq![Effect::Complete(Complete::Retryable)]);
    let (_, closed) = decide(prepared, QueueEvent::LabelRead { head: Head::Current, open: false, merged: false });
    assert(closed@ == seq![Effect::Error(Failure::PullClosed)]);
    let (_, label) = decide(prepared, QueueEvent::LabelRead { head: Head::Current, open: true, merged: false });
    assert(label@ == seq![Effect::Write(Write::Label)]);
 }
}
