use crate::enqueue::*;
use crate::enqueue_helpers::*;
use crate::{Head, QueueState};
use vstd::prelude::*;

verus! {
/// All four enqueue_refused_sticky covers, each through the real decide.
fn covers() {
    let state = EnqueueState { intent: Some(Intent { requeue: Some(Requeue::Refused) }),
        confirmed: false, recovery: None, rerun: None };
    let (prepared, a) = decide(state, QueueEvent::Prepare {
        dequeue_pending: false, checks_ready: true, queue_label: false });
    assert(prepared.intent == state.intent && a@.len() == 2);
    let (ticked, b) = decide(state, QueueEvent::RecoveryTick { eligible: true, now_ms: 0 });
    assert(ticked.recovery.unwrap().command == Command::Refused && b@[0] == Effect::SaveRecovery);
    let read = QueueRead { head: Head::Current, queue: QueueState::Waiting, merged: false,
        conflicts: false, held: false, check_changed: false, refusal: None,
        reruns_pending: None, person_removed: None, issue: None };
    let (reconciled, c) = decide(state, QueueEvent::Reconcile(read));
    assert(reconciled.intent == state.intent && c@[1] == Effect::Complete(Complete::Permanent(Failure::Refused)));
    let recovery = Recovery { command: Command::Refused, eligible_since_ms: Some(123) };
    let terminal = EnqueueState { intent: None, confirmed: false, recovery: Some(recovery), rerun: None };
    let (late, d) = decide(terminal, QueueEvent::CommandDone(CommandResult::Accepted));
    assert(late.recovery == Some(recovery));
    assert(d@[d@.len() - 1] == Effect::Complete(Complete::Permanent(Failure::Refused)));
    assert(no_write(d@));
}
}
