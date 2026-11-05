use crate::enqueue::*;
use crate::enqueue_helpers::*;
use crate::Head;
use vstd::prelude::*;

verus! {
/// TL3 #608 requires about five minutes of eligibility and one command per head;
/// #751 K-recovery-grace requires the grace window and only-once dispatch.
/// #760 explicitly forbids rechecking grace against the later budget reread.
/// Infrastructure recovery context: docs/state-table.md:541,575, not a timer rule.
/// Served incident: RobTand/prismabuild#2705 as recorded in #608.
/// Exact timer encoding: preserved existing behaviour (no rule).
/// Both Option<Recovery> samples and every u64 timestamp remain arbitrary.
pub open spec fn contract(old: EnqueueState, event: QueueEvent, next: EnqueueState, effects: Effects) -> bool {
    !refused(old) ==> match event {
        QueueEvent::RecoveryTick { eligible, now_ms } => if ready(old.recovery) {
            next.intent == old.intent
            && next.recovery == Some(Recovery { command: Command::Ready,
                eligible_since_ms: if eligible { Some(timer(old.recovery, now_ms)) } else { None } })
            && effects@ == Seq::empty().push(Effect::SaveRecovery).push(if eligible && now_ms.saturating_sub(timer(old.recovery, now_ms)) >= GRACE_MS {
                    Effect::Read(Need::Command)
                } else { Effect::Complete(Complete::Pending) })
            && GRACE_MS == 300000
        } else {
            next.recovery == old.recovery && no_write(effects@)
            && effects@ == seq![Effect::Complete(completion(old.recovery.unwrap().command))]
        },
        QueueEvent::DispatchCommand(read) => {
            next.intent == old.intent
            && effects@ == (if !ready(old.recovery) {
                seq![Effect::Complete(completion(old.recovery.unwrap().command))]
            } else if read.waiting && !read.eligible {
                Seq::empty().push(Effect::SaveRecovery).push(Effect::Complete(Complete::Pending))
            } else if read.head != Head::Current || !read.open || read.merged
                || read.draft || read.held || read.paused {
                seq![Effect::Complete(Complete::Pending)]
            } else { Seq::empty().push(Effect::SaveRecovery).push(Effect::Write(Write::Command)) })
            && ((ready(old.recovery) && (!read.waiting || read.eligible)
                && read.head == Head::Current && read.open && !read.merged
                && !read.draft && !read.held && !read.paused) ==> {
                next.recovery.is_some() && next.recovery.unwrap().command == Command::Sending
            })
            && (!ready(old.recovery) ==> no_write(effects@))
        },
        _ => true,
    }
}
}
