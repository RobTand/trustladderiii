use crate::enqueue::*;
use crate::enqueue_helpers::*;
use crate::Head;
use vstd::prelude::*;

verus! {
/// TL3 #751 K-write-ahead; #608 requires durable intent before command dispatch
/// and fresh exact-head/eligibility checks. #760 preserves that I/O order.
/// Recovery context: docs/state-table.md:541,575. Q41 exact reviewed commit is
/// docs/state-machine.md:132,487. Served incident: RobTand/prismabuild#2705 in #608.
/// Immediate predecessor/receipt encoding: preserved existing behaviour (no rule).
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
