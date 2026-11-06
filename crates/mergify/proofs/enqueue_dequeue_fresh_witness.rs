use crate::enqueue::*;
use crate::dequeue_contract::{expected, fresh};
use crate::Time;
use vstd::prelude::*;

verus! {
/// Every assertion from enqueue_dequeue_fresh over all original arbitrary inputs.
fn equivalence(retry: bool, concluded: bool, labeled: bool,
    dequeued: Option<LabelEvent>, queued: Option<u64>, finished: Option<Time>) {
    let pending = dequeue_pending(retry, concluded, labeled, dequeued, queued, finished);
    assert(pending == expected(retry, concluded, labeled, dequeued, queued, finished));
}

/// Original cover: pending and no label yet.
fn waiting_for_label() {
    let pending = dequeue_pending(true, false, false, None, None, None);
    assert(pending);
}

/// Original cover: a label exists, but its event belongs to the old cycle.
/// Equal IDs are stale even when the event timestamp is later than completion.
fn old_cycle() {
    let event = LabelEvent { id: 7, labeled: true, at: Time { secs: 1, nanos: 0 } };
    let finished = Time { secs: -1, nanos: 0 };
    let pending = dequeue_pending(true, false, true, Some(event), Some(7), Some(finished));
    assert(!fresh(Some(event), Some(7), Some(finished)) && pending);
}

/// Original cover: a fresh event finishes dequeue. Equal completion time is
/// fresh, while an equal second with one earlier nanosecond remains stale.
fn fresh_event() {
    let event = LabelEvent { id: 7, labeled: true, at: Time { secs: -1, nanos: 1000000000 } };
    let finished = Time { secs: -1, nanos: 1000000000 };
    let pending = dequeue_pending(true, false, true, Some(event), None, Some(finished));
    assert(fresh(Some(event), None, Some(finished)) && !pending);
    let earlier = LabelEvent { at: Time { secs: -1, nanos: 999999999 }, ..event };
    let waiting = dequeue_pending(true, false, true, Some(earlier), None, Some(finished));
    assert(!fresh(Some(earlier), None, Some(finished)) && waiting);
}
}
