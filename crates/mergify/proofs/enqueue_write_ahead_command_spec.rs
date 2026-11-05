use crate::enqueue::*;
use crate::enqueue_helpers::*;
use crate::Head;
use vstd::prelude::*;

verus! {
/// Publishing/Queued, docs/state-table.md:537-546,573-580; Q41/Q47/Q52;
/// #608/#751/#760/#2705. The save immediately precedes every command write.
/// Full typed input domains and the actual Vec; no bound or trusted body.
pub open spec fn contract(old: EnqueueState, event: QueueEvent, next: EnqueueState, effects: Effects) -> bool {
    ((!refused(old) && (match event { QueueEvent::DispatchCommand(read) => ready(old.recovery)
        && (!read.waiting || read.eligible) && read.head == Head::Current && read.open
        && !read.merged && !read.draft && !read.held && !read.paused, _ => false })) ==>
        effects@ == Seq::empty().push(Effect::SaveRecovery).push(Effect::Write(Write::Command)))
    && forall|i: int| 0 <= i < effects@.len() && #[trigger] effects@[i] == Effect::Write(Write::Command) ==> {
        i > 0 && effects@[i - 1] == Effect::SaveRecovery
        && match next.recovery { Some(r) => r.command == Command::Sending, None => false }
    }
}
}
