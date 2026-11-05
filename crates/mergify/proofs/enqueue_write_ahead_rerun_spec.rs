use crate::enqueue::*;
use crate::enqueue_helpers::*;
use crate::Head;
use vstd::prelude::*;

verus! {
/// TL3 #751 K-write-ahead requires recorded intent before every upstream write.
/// Infrastructure rerun context: Q47, docs/state-machine.md:681-688 and
/// docs/state-table.md:541,575; these do not define receipt ordering or encoding.
/// #760 preserves the shell; #608 records served incident RobTand/prismabuild#2705.
/// Immediate predecessor/receipt encoding: preserved existing behaviour (no rule).
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
