use crate::enqueue::*;
use crate::enqueue_helpers::*;
use vstd::prelude::*;

verus! {
/// Publishing/Queued, docs/state-table.md:537-546,573-580; Q41/Q47;
/// #608/#751/#760/#2705: terminal refusals survive every late recovery outcome.
/// Full typed domains; no loop, unwind bound, assumption, or external body.
pub open spec fn contract(old: EnqueueState, event: QueueEvent, next: EnqueueState, effects: Effects) -> bool {
    (refused(old) ==> {
        next.intent == old.intent
        && effects@ == Seq::empty().push(Effect::SaveRecovery).push(Effect::Complete(Complete::Permanent(Failure::Refused)))
        && next.recovery == Some(Recovery { eligible_since_ms: None, command: Command::Refused })
    }) && (!refused(old) ==> match old.recovery {
        Some(r) => if matches!(r.command, Command::Refused | Command::PermissionDenied | Command::Rejected)
            && matches!(event, QueueEvent::CommandStatus | QueueEvent::CommandReceipt { .. }
                | QueueEvent::DispatchCommand(_) | QueueEvent::CommandDone(_) | QueueEvent::RecoveryTick { .. }) {
            next.recovery == old.recovery
            && effects@.len() > 0
            && effects@[effects@.len() - 1] == Effect::Complete(completion(r.command))
            && no_write(effects@)
        } else { true },
        None => true,
    })
}
}
