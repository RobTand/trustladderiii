//! Contracts for the actual helpers called by enqueue::decide.
use crate::enqueue::*;
use crate::QueueState;
use vstd::prelude::*;

verus! {
pub open spec fn completion(command: Command) -> Complete {
    match command {
        Command::Refused => Complete::Permanent(Failure::Refused),
        Command::PermissionDenied => Complete::Permanent(Failure::Permission),
        Command::Rejected => Complete::Permanent(Failure::Rejected),
        _ => Complete::Pending,
    }
}

/// Publishing enqueue, docs/state-table.md:537-546; Q41, #608.
/// The helper only requests another read or completes; it never writes.
pub open spec fn issue(intent: Option<Intent>, queue: QueueState, read: IssueRead, result: Effect) -> bool {
    !matches!(result, Effect::Write(_))
}

pub open spec fn refused(state: EnqueueState) -> bool {
    state.intent == Some(Intent { requeue: Some(Requeue::Refused) })
}

pub open spec fn ready(recovery: Option<Recovery>) -> bool {
    match recovery { None => true, Some(r) => r.command == Command::Ready }
}

pub open spec fn timer(recovery: Option<Recovery>, now_ms: u64) -> u64 {
    match recovery { Some(Recovery { eligible_since_ms: Some(since), .. }) => since, _ => now_ms }
}

pub open spec fn elapsed(now_ms: u64, since: u64) -> int {
    if now_ms >= since { now_ms as int - since as int } else { 0 }
}

pub open spec fn no_write(effects: Seq<Effect>) -> bool {
    forall|i: int| 0 <= i < effects.len() ==> match #[trigger] effects[i] { Effect::Write(_) => false, _ => true }
}
}
