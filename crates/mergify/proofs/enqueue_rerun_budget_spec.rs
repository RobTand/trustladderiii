use crate::enqueue::*;
use crate::enqueue_helpers::*;
use crate::{Head, Status};
use vstd::prelude::*;

verus! {
/// Q47, docs/state-machine.md:681-688, state-table.md:541,549,575:
/// runner/setup failures use the existing infrastructure retry path.
/// #447/state-table.md:545,578 concern unmet queue conditions, not receipts.
/// TL3 #608 records the served RobTand/prismabuild#2705 incident; that PR's
/// public-evidence change does not define rerun budgets. Receipt encoding and
/// result mapping here are preserved existing behaviour (no rule), base
/// 504f10b0 enqueue.rs:631-703. Full u64/typed state domains, no temporal bound.
pub open spec fn read_outcome(old: Option<Rerun>, head: Head, attempt: u64,
    status: Status, writable: Option<bool>) -> (Option<Rerun>, Seq<Effect>) {
    if head != Head::Current {
        (old, seq![Effect::Error(Failure::PullChanged)])
    } else if match old { Some(r) => r.accepted || attempt > r.attempt, None => false } {
        (old, seq![Effect::Continue])
    } else if match old { Some(r) => !r.waiting, None => false } {
        (old, seq![Effect::Error(Failure::RerunUnknown)])
    } else if status != Status::Completed {
        (Some(Rerun { attempt, accepted: false, waiting: true }),
            seq![Effect::SaveRerun, Effect::Error(Failure::RerunFinishing)])
    } else if writable == None {
        (old, seq![Effect::Read(Need::RerunPull)])
    } else if writable == Some(false) {
        (old, seq![Effect::Error(Failure::PullChanged)])
    } else {
        (Some(Rerun { attempt, accepted: false, waiting: false }),
            seq![Effect::SaveRerun, Effect::Write(Write::Rerun)])
    }
}

pub open spec fn done_receipt(old: Option<Rerun>, result: RerunResult) -> Option<Rerun> {
    match result {
        RerunResult::Accepted => match old {
            Some(r) => Some(Rerun { accepted: true, ..r }), None => None },
        RerunResult::InProgress => match old {
            Some(r) => Some(Rerun { waiting: true, ..r }), None => None },
        RerunResult::Refused => None,
        RerunResult::Unknown => old,
    }
}

pub open spec fn pending(old: Option<Rerun>, head: Head, attempt: u64, status: Status) -> bool {
    match old { None => false, Some(r) => !r.accepted &&
        (head != Head::Current || attempt <= r.attempt && (!r.waiting || status != Status::Completed)) }
}

pub open spec fn contract(old: EnqueueState, event: QueueEvent, next: EnqueueState, effects: Effects) -> bool {
    (match event {
        QueueEvent::RerunRead { .. } | QueueEvent::RerunDone(_) | QueueEvent::RerunPending { .. } => {
            next.intent == old.intent && next.confirmed == old.confirmed &&
            if refused(old) {
                next.rerun == old.rerun &&
                next.recovery == Some(Recovery { eligible_since_ms: None, command: Command::Refused }) &&
                effects@ == seq![Effect::SaveRecovery, Effect::Complete(Complete::Permanent(Failure::Refused))]
            } else {
                next.recovery == old.recovery && match event {
                    QueueEvent::RerunRead { head, attempt, status, writable } => {
                        let expected = read_outcome(old.rerun, head, attempt, status, writable);
                        next.rerun == expected.0 && effects@ == expected.1
                    },
                    QueueEvent::RerunDone(result) => next.rerun == done_receipt(old.rerun, result)
                        && effects@ == seq![Effect::SaveRerun],
                    QueueEvent::RerunPending { head, attempt, status } => next == old &&
                        effects@ == seq![Effect::RerunPending(pending(old.rerun, head, attempt, status))],
                    _ => true,
                }
            }
        },
        _ => true,
    }) && forall|i: int| 0 <= i < effects@.len() && #[trigger] effects@[i] == Effect::Write(Write::Rerun) ==>
        i > 0 && effects@[i - 1] == Effect::SaveRerun &&
        match next.rerun { Some(r) => !r.accepted && !r.waiting, None => false }
}
}
