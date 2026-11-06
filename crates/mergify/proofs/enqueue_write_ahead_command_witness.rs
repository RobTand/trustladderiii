use crate::enqueue::*;
use crate::Head;
use vstd::prelude::*;

verus! {
/// Original enqueue_write_ahead cover: a reachable command write with its save.
fn cover() {
    let state = EnqueueState { intent: None, confirmed: false, recovery: None, rerun: None };
    let (_, effects) = decide(state, QueueEvent::DispatchCommand(CommandRead { head: Head::Current,
        open: true, merged: false, draft: false, held: false, paused: false,
        waiting: false, eligible: false, now_ms: 0 }));
    assert(effects@ == Seq::empty().push(Effect::SaveRecovery).push(Effect::Write(Write::Command)));
}
}
