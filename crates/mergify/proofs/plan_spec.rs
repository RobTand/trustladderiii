use vstd::prelude::*;
use crate::plan::*;
verus! {
/// #751 write-ahead and #760 I/O-order fidelity. No bounded loop.
fn intent_witness() {
    let plan = intent();
    assert(plan.0 == IntentEffect::Save && plan.1 == IntentEffect::Fence);
    let step = next::<u8>(Some(7));
    assert(matches!(step, Next::Effect(7)));
    let done = next::<u8>(None);
    assert(matches!(done, Next::Applied));
}
/// Preserved existing behaviour (no rule): fence equality is accepted.
fn fence_witness() {
    let old = fenced(1, 2);
    assert(old);
    let same = fenced(2, 2);
    assert(!same);
    let newer = fenced(3, 2);
    assert(!newer);
}
/// Preserved existing behaviour (no rule): each receipt and optional payload path is reachable.
fn receipt_witness() {
    let confirmed = start(Receipt::Found { value: 7u8, ack: Ack::Confirmed });
    assert(matches!(confirmed, Start::Found { value: 7, read_receipt: true }));
    let other = start(Receipt::Found { value: 8u8, ack: Ack::Other });
    assert(matches!(other, Start::Found { value: 8, read_receipt: false }));
    let pending = start::<u8>(Receipt::Pending);
    assert(matches!(pending, Start::Pending));
    let absent = start::<u8>(Receipt::Absent);
    assert(matches!(absent, Start::Proceed));
    let deliver = needs_delivery(false);
    assert(deliver);
    let skip = needs_delivery(true);
    assert(!skip);
    let found = found(Receipt::Found { value: 9u8, ack: Ack::Other });
    assert(found == Some(9));
    let missing = crate::plan::found::<u8>(Receipt::Pending);
    assert(missing == None);
    let final_ = finish(Receipt::Found { value: 1u8, ack: Ack::Confirmed });
    assert(matches!(final_, Final::Found { value: 1, deliver: true }));
    let unknown = finish::<u8>(Receipt::Absent);
    assert(matches!(unknown, Final::Unknown));
}
/// Preserved existing behaviour (no rule): saved attempts and nonempty event collections reach production.
fn attempt_witness() {
    let saved = attempt(Some(3u8));
    assert(matches!(saved, Attempt::Saved(3)));
    let new = attempt::<u8>(None);
    assert(matches!(new, Attempt::New));
    let requeue = ready(true, 4u8);
    assert(requeue == Some(4));
    let absent = ready(false, 4u8);
    assert(absent == None);
    let max = last_event(&vec![0, 9, 4, 9]);
    assert(max == 9);
    let empty = last_event(&vec![]);
    assert(empty == 0);
}
}
