use crate::enqueue::*;
use crate::enqueue_contract::stale;
use crate::enqueue_helpers::refused;
use crate::{Head, QueueState};
use vstd::prelude::*;

verus! {
/// Original covers: stale reconcile retries; held stale head waits.
fn reconcile_covers() {
    let state = EnqueueState { intent: None, confirmed: false, recovery: None, rerun: None };
    let read = QueueRead { head: Head::Other, queue: QueueState::Waiting, merged: false,
        conflicts: false, held: false, check_changed: false, refusal: None, reruns_pending: Some(false),
        person_removed: None, issue: None };
    let event = QueueEvent::Reconcile(read);
    let (_, effects) = decide(state, event);
    assert(stale(state, event) && effects@[0] == Effect::Complete(Complete::Retryable));
    let held_event = QueueEvent::Reconcile(QueueRead { held: true, ..read });
    let (_, held) = decide(state, held_event);
    assert(stale(state, held_event) && held@[0] == Effect::Complete(Complete::Pending));
}

/// Original cover: stale command blocked, with no refused intent.
fn command_cover() {
    let state = EnqueueState { intent: None, confirmed: false, recovery: None, rerun: None };
    let event = QueueEvent::DispatchCommand(CommandRead { head: Head::Other, open: true, merged: false,
        draft: false, held: false, paused: false, waiting: false, eligible: true, now_ms: 0 });
    let (_, effects) = decide(state, event);
    assert(stale(state, event) && !refused(state));
    assert(forall|i: int| 0 <= i < effects@.len() ==> match #[trigger] effects@[i] { Effect::Write(_) => false, _ => true });
}
}
