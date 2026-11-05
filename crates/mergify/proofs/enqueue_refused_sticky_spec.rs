use crate::enqueue::*;
use crate::enqueue_helpers::*;
use vstd::prelude::*;

verus! {
/// TL3 #751 K-refused-sticky: a refusal is never silently retried; #608
/// retains existing refusal handling. Exact terminal receipt/timer values are
/// preserved existing behaviour (no rule). Context: infrastructure recovery in
/// docs/state-table.md:541,575; neither row defines terminal receipt encoding.
/// #760 preserves the lifecycle; served incident RobTand/prismabuild#2705 is
/// identified by #608, not a TL3 issue or an additional rule.
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
