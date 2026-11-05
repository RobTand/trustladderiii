use crate::enqueue::*;
use crate::enqueue_helpers::*;
use crate::Head;
use vstd::prelude::*;

verus! {
/// Publishing/Queued, docs/state-table.md:537-546,573-580; Q41/Q47/Q52;
/// #608/#751/#760/#2705. The save immediately precedes every rerun write.
/// Full typed input domains and the actual Vec; no bound or trusted body.
pub open spec fn contract(old: EnqueueState, event: QueueEvent, next: EnqueueState, effects: Effects) -> bool {
    ((!refused(old) && (match event { QueueEvent::RerunRead { head, status, writable, .. } =>
        head == Head::Current && old.rerun.is_none() && status == crate::Status::Completed
        && writable == Some(true), _ => false })) ==>
        effects@ == Seq::empty().push(Effect::SaveRerun).push(Effect::Write(Write::Rerun)))
    && forall|i: int| 0 <= i < effects@.len() && #[trigger] effects@[i] == Effect::Write(Write::Rerun) ==> {
        i > 0 && effects@[i - 1] == Effect::SaveRerun
        && match next.rerun { Some(r) => !r.accepted && !r.waiting, None => false }
    }
}
}
