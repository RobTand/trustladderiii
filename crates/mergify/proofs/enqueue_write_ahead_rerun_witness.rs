use crate::enqueue::*;
use crate::Head;
use vstd::prelude::*;

verus! {
/// Original enqueue_write_ahead cover: a reachable rerun write with its save.
fn cover() {
    let state = EnqueueState { intent: None, confirmed: false, recovery: None, rerun: None };
    let (_, effects) = decide(state, QueueEvent::RerunRead { head: Head::Current, attempt: 0,
        status: crate::Status::Completed, writable: Some(true) });
    assert(effects@ == Seq::empty().push(Effect::SaveRerun).push(Effect::Write(Write::Rerun)));
}
}
