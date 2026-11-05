//! Shared command protocol for legacy requeue and repaired-head recovery callers.
//! The shell owns the head key, sampling, fence, persistence, and upstream calls.
use super::Head;
use super::enqueue::{
    Command, CommandRead, CommandResult, Complete, Effect, Failure, GRACE_MS, Need, Recovery, Write,
};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// The shell must consume `first` before `second`, including a Sending save before POST.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub recovery: Option<Recovery>,
    pub first: Option<Effect>,
    pub second: Option<Effect>,
}

/// #608: terminal refusals retain their failure class. Unbounded over Command.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == completion_spec(command)))]
pub fn completion(command: Command) -> Complete {
    match command {
        Command::Refused => Complete::Permanent(Failure::Refused),
        Command::PermissionDenied => Complete::Permanent(Failure::Permission),
        Command::Rejected => Complete::Permanent(Failure::Rejected),
        Command::Ready | Command::Sending | Command::Sent => Complete::Pending,
    }
}

/// #608: a lost response requests reconciliation, never another command.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == status_spec(recovery)))]
pub fn status(recovery: Option<Recovery>) -> Effect {
    let command = match recovery {
        Some(saved) => saved.command,
        None => Command::Ready,
    };
    match command {
        Command::Ready => Effect::Continue,
        Command::Sending | Command::Sent => Effect::Read(Need::CommandReply),
        _ => Effect::Complete(completion(command)),
    }
}

/// #608/#760: grace uses the shell's captured clock, over every u64 timestamp.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == tick_spec(recovery, eligible, now_ms)))]
pub fn tick(recovery: Option<Recovery>, eligible: bool, now_ms: u64) -> Transition {
    let mut saved = match recovery {
        Some(saved) => saved,
        None => Recovery {
            eligible_since_ms: None,
            command: Command::Ready,
        },
    };
    if !matches!(saved.command, Command::Ready) {
        return Transition {
            recovery: Some(saved),
            first: Some(Effect::Complete(completion(saved.command))),
            second: None,
        };
    }
    if !eligible {
        saved.eligible_since_ms = None;
    }
    let matured = if eligible {
        let since = match saved.eligible_since_ms {
            Some(since) => since,
            None => now_ms,
        };
        saved.eligible_since_ms = Some(since);
        now_ms.saturating_sub(since) >= GRACE_MS
    } else {
        false
    };
    Transition {
        recovery: Some(saved),
        first: Some(Effect::SaveRecovery),
        second: Some(if matured {
            Effect::Read(Need::Command)
        } else {
            Effect::Complete(Complete::Pending)
        }),
    }
}

/// #608: refusal wins over a found comment; absence cannot erase Sending.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == receipt_spec(recovery, refused, found)))]
pub fn receipt(recovery: Option<Recovery>, refused: bool, found: bool) -> Transition {
    let mut saved = match recovery {
        Some(saved) => saved,
        None => Recovery {
            eligible_since_ms: None,
            command: Command::Sending,
        },
    };
    let in_flight = matches!(saved.command, Command::Sending | Command::Sent);
    if in_flight && refused {
        saved.command = Command::Refused;
    } else if in_flight && found {
        saved.command = Command::Sent;
    }
    let complete = Some(Effect::Complete(completion(saved.command)));
    if in_flight && (refused || found) {
        Transition {
            recovery: Some(saved),
            first: Some(Effect::SaveRecovery),
            second: complete,
        }
    } else {
        Transition {
            recovery: Some(saved),
            first: complete,
            second: None,
        }
    }
}

/// #608/#751: both callers persist Sending before POST, and honor human holds.
/// The repaired-head caller supplies the prior grace grant; this step never resamples it.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == dispatch_spec(recovery, read)))]
pub fn dispatch(recovery: Option<Recovery>, read: CommandRead) -> Transition {
    let mut saved = match recovery {
        Some(saved) => saved,
        None => Recovery {
            eligible_since_ms: None,
            command: Command::Ready,
        },
    };
    if !matches!(saved.command, Command::Ready) {
        Transition {
            recovery: Some(saved),
            first: Some(Effect::Complete(completion(saved.command))),
            second: None,
        }
    } else if read.waiting && !read.eligible {
        saved.eligible_since_ms = None;
        Transition {
            recovery: Some(saved),
            first: Some(Effect::SaveRecovery),
            second: Some(Effect::Complete(Complete::Pending)),
        }
    } else if !matches!(read.head, Head::Current)
        || !read.open
        || read.merged
        || read.draft
        || read.held
        || read.paused
    {
        Transition {
            recovery: Some(saved),
            first: Some(Effect::Complete(Complete::Pending)),
            second: None,
        }
    } else {
        saved.command = Command::Sending;
        Transition {
            recovery: Some(saved),
            first: Some(Effect::SaveRecovery),
            second: Some(Effect::Write(Write::Command)),
        }
    }
}

/// #608: transport uncertainty consumes the budget; terminal outcomes are sticky.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == done_spec(recovery, result)))]
pub fn done(recovery: Option<Recovery>, result: CommandResult) -> Transition {
    let Some(mut saved) = recovery else {
        return Transition {
            recovery: None,
            first: None,
            second: None,
        };
    };
    if !matches!(saved.command, Command::Sending) {
        return Transition {
            recovery: Some(saved),
            first: Some(Effect::Complete(completion(saved.command))),
            second: None,
        };
    }
    saved.command = match result {
        CommandResult::Accepted => Command::Sent,
        CommandResult::PermissionDenied => Command::PermissionDenied,
        CommandResult::Rejected => Command::Rejected,
        CommandResult::Unknown => saved.command,
    };
    Transition {
        recovery: Some(saved),
        first: Some(Effect::SaveRecovery),
        second: Some(Effect::Complete(completion(saved.command))),
    }
}

/// State-table Queued/person removal: only a verified newer resume releases a hold.
/// `unresolved` is the existing QueueFact::unresolved_hold snapshot projection.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (!snapshot_present || unresolved)))]
pub fn human_hold(snapshot_present: bool, unresolved: bool) -> bool {
    !snapshot_present || unresolved
}

#[cfg(verus_keep_ghost)]
verus! {
pub open spec fn completion_spec(command: Command) -> Complete {
    match command {
        Command::Refused => Complete::Permanent(Failure::Refused),
        Command::PermissionDenied => Complete::Permanent(Failure::Permission),
        Command::Rejected => Complete::Permanent(Failure::Rejected),
        _ => Complete::Pending,
    }
}
pub open spec fn ready(recovery: Option<Recovery>) -> Recovery {
    match recovery { Some(saved) => saved, None => Recovery { eligible_since_ms: None, command: Command::Ready } }
}
pub open spec fn status_spec(recovery: Option<Recovery>) -> Effect {
    match ready(recovery).command {
        Command::Ready => Effect::Continue,
        Command::Sending | Command::Sent => Effect::Read(Need::CommandReply),
        command => Effect::Complete(completion_spec(command)),
    }
}
pub open spec fn matured(since: u64, now_ms: u64) -> bool {
    (now_ms as int) >= (since as int) + 300_000
}
pub open spec fn tick_spec(recovery: Option<Recovery>, eligible: bool, now_ms: u64) -> Transition {
    let saved = ready(recovery);
    if saved.command != Command::Ready {
        Transition { recovery: Some(saved), first: Some(Effect::Complete(completion_spec(saved.command))), second: None }
    } else {
        let since = match saved.eligible_since_ms { Some(since) => since, None => now_ms };
        Transition {
            recovery: Some(Recovery { command: saved.command, eligible_since_ms: if eligible { Some(since) } else { None } }),
            first: Some(Effect::SaveRecovery),
            second: Some(if eligible && matured(since, now_ms) { Effect::Read(Need::Command) } else { Effect::Complete(Complete::Pending) }),
        }
    }
}
pub open spec fn receipt_spec(recovery: Option<Recovery>, refused: bool, found: bool) -> Transition {
    let saved = match recovery { Some(saved) => saved, None => Recovery { eligible_since_ms: None, command: Command::Sending } };
    let in_flight = matches!(saved.command, Command::Sending | Command::Sent);
    let command = if in_flight && refused { Command::Refused } else if in_flight && found { Command::Sent } else { saved.command };
    let complete = Some(Effect::Complete(completion_spec(command)));
    Transition { recovery: Some(Recovery { command, ..saved }), first: if in_flight && (refused || found) { Some(Effect::SaveRecovery) } else { complete }, second: if in_flight && (refused || found) { complete } else { None } }
}
pub open spec fn writable(read: CommandRead) -> bool {
    read.head == Head::Current && read.open && !read.merged && !read.draft && !read.held && !read.paused
}
pub open spec fn dispatch_spec(recovery: Option<Recovery>, read: CommandRead) -> Transition {
    let saved = ready(recovery);
    if saved.command != Command::Ready {
        Transition { recovery: Some(saved), first: Some(Effect::Complete(completion_spec(saved.command))), second: None }
    } else if read.waiting && !read.eligible {
        Transition { recovery: Some(Recovery { eligible_since_ms: None, ..saved }), first: Some(Effect::SaveRecovery), second: Some(Effect::Complete(Complete::Pending)) }
    } else if !writable(read) {
        Transition { recovery: Some(saved), first: Some(Effect::Complete(Complete::Pending)), second: None }
    } else {
        Transition { recovery: Some(Recovery { command: Command::Sending, ..saved }), first: Some(Effect::SaveRecovery), second: Some(Effect::Write(Write::Command)) }
    }
}
pub open spec fn done_spec(recovery: Option<Recovery>, result: CommandResult) -> Transition {
    match recovery {
        None => Transition { recovery: None, first: None, second: None },
        Some(saved) => {
            if saved.command != Command::Sending {
                Transition { recovery: Some(saved), first: Some(Effect::Complete(completion_spec(saved.command))), second: None }
            } else {
                let command = match result { CommandResult::Accepted => Command::Sent, CommandResult::PermissionDenied => Command::PermissionDenied, CommandResult::Rejected => Command::Rejected, CommandResult::Unknown => Command::Sending };
                Transition { recovery: Some(Recovery { command, ..saved }), first: Some(Effect::SaveRecovery), second: Some(Effect::Complete(completion_spec(command))) }
            }
        }
    }
}
pub open spec fn posts(step: Transition) -> bool {
    step.first == Some(Effect::Write(Write::Command)) || step.second == Some(Effect::Write(Write::Command))
}
}

#[cfg(verus_keep_ghost)]
mod proof_one_command {
    use super::*;
    verus! {
    /// #608/#751: an induction step for any key whose Sending receipt is durable.
    /// Unbounded scalar domains and arbitrary future step count by induction.
    /// The caller has passed CC-M1's Reconcile/reconcile_issue recovery-file gate.
    fn consumed_budget(command: Command, since: Option<u64>, read: CommandRead,
        refused: bool, found: bool, result: CommandResult, eligible: bool, now_ms: u64,
        recovery_file_gate: bool)
        requires recovery_file_gate, command != Command::Ready,
    {
        let saved = Some(Recovery { command, eligible_since_ms: since });
        let dispatch = dispatch(saved, read);
        let receipt = receipt(saved, refused, found);
        let done = done(saved, result);
        let tick = tick(saved, eligible, now_ms);
        assert(!posts(dispatch) && !posts(receipt) && !posts(done) && !posts(tick));
        assert(ready(dispatch.recovery).command != Command::Ready);
        assert(ready(receipt.recovery).command != Command::Ready);
        assert(ready(done.recovery).command != Command::Ready);
        assert(ready(tick.recovery).command != Command::Ready);
    }
    /// #608: reachable successful dispatch, followed by a lost response and restart.
    fn sending_before_post_witness() {
        let read = CommandRead { head: Head::Current, open: true, merged: false,
            draft: false, held: false, paused: false, waiting: false,
            eligible: false, now_ms: 0 };
        let first = dispatch(None, read);
        assert(first.first == Some(Effect::SaveRecovery));
        assert(first.second == Some(Effect::Write(Write::Command)));
        assert(ready(first.recovery).command == Command::Sending);
        let lost = done(first.recovery, CommandResult::Unknown);
        assert(ready(lost.recovery).command == Command::Sending);
        let restart = dispatch(lost.recovery, read);
        assert(!posts(restart));
        let response = status(lost.recovery);
        assert(response == Effect::Read(Need::CommandReply));
        consumed_budget(Command::Sending, None, read, false, false,
            CommandResult::Unknown, true, 0, true);
    }
    }
}

#[cfg(verus_keep_ghost)]
mod proof_refused {
    use super::*;
    verus! {
    /// #608: all three terminal refusal variants survive every command transition.
    /// Unbounded u64 time and all event payloads; one-step induction, no trace bound.
    fn terminal_is_sticky(command: Command, since: Option<u64>, read: CommandRead,
        refused: bool, found: bool, result: CommandResult, eligible: bool, now_ms: u64)
        requires matches!(command, Command::Refused | Command::PermissionDenied | Command::Rejected),
    {
        let saved = Some(Recovery { command, eligible_since_ms: since });
        let a = dispatch(saved, read);
        let b = receipt(saved, refused, found);
        let c = done(saved, result);
        let d = tick(saved, eligible, now_ms);
        assert(a.recovery == saved && b.recovery == saved && c.recovery == saved && d.recovery == saved);
        assert(!posts(a) && !posts(b) && !posts(c) && !posts(d));
        let response = status(saved);
        assert(response == Effect::Complete(completion_spec(command)));
    }
    /// #608: each terminal variant is reachable through its real production transition.
    fn terminal_witnesses() {
        let sending = Some(Recovery { command: Command::Sending, eligible_since_ms: None });
        let refused = receipt(sending, true, true);
        let permission = done(sending, CommandResult::PermissionDenied);
        let rejected = done(sending, CommandResult::Rejected);
        assert(ready(refused.recovery).command == Command::Refused);
        assert(ready(permission.recovery).command == Command::PermissionDenied);
        assert(ready(rejected.recovery).command == Command::Rejected);
    }
    }
}

#[cfg(verus_keep_ghost)]
mod proof_grace {
    use super::*;
    verus! {
    /// #608/#760: repaired-head dispatch needs the earlier sampled grace grant.
    /// The later budget reread may change the timer but cannot revoke that grant.
    /// CC-M1's Reconcile/reconcile_issue gate outcome is an explicit precondition.
    /// Unbounded u64 timestamps and independent head records; no bounded trace.
    fn sampled_grace(since: Option<u64>, now_ms: u64, reread: Option<Recovery>,
        recovery_file_gate: bool)
        requires recovery_file_gate,
    {
        let initial = Some(Recovery { command: Command::Ready, eligible_since_ms: since });
        let grant = tick(initial, true, now_ms);
        if matches!(grant.second, Some(Effect::Read(Need::Command))) {
            assert(since.is_some());
            assert((now_ms as int) >= (since.unwrap() as int) + 300_000);
            let read = CommandRead { head: Head::Current, open: true, merged: false,
                draft: false, held: false, paused: false, waiting: true,
                eligible: true, now_ms };
            let sent = dispatch(reread, read);
            assert(posts(sent) == (ready(reread).command == Command::Ready));
        }
    }
    /// #608: grace starts, matures exactly at its boundary, and resets on ineligibility.
    /// Independent value arguments represent distinct head-keyed records from the shell.
    fn per_head_witness() {
        let first = tick(None, true, 7);
        assert(!posts(first));
        assert(first.second == Some(Effect::Complete(Complete::Pending)));
        let early = tick(first.recovery, true, 300_006);
        assert(early.second == Some(Effect::Complete(Complete::Pending)));
        let matured = tick(first.recovery, true, 300_007);
        assert(matured.second == Some(Effect::Read(Need::Command)));
        let other_head = tick(None, true, 300_007);
        assert(other_head.second == Some(Effect::Complete(Complete::Pending)));
        let reset = tick(first.recovery, false, 300_007);
        assert(ready(reset.recovery).eligible_since_ms.is_none());
        sampled_grace(Some(7), 300_007, None, true);
    }
    }
}

#[cfg(verus_keep_ghost)]
mod proof_human_hold {
    use super::*;
    verus! {
    /// State-table Queued/person removal: an unresolved dequeue or close blocks POST.
    /// `unresolved` comes from QueueFact::unresolved_hold: only a newer resume clears it.
    /// All Boolean inputs and arbitrary recovery state; no sequence or timestamp bound.
    fn person_blocks(recovery: Option<Recovery>, snapshot: bool, unresolved: bool,
        waiting: bool, eligible: bool)
        requires !snapshot || unresolved,
    {
        let held = human_hold(snapshot, unresolved);
        assert(held);
        let read = CommandRead { head: Head::Current, open: true, merged: false,
            draft: false, held, paused: false, waiting, eligible, now_ms: 0 };
        let result = dispatch(recovery, read);
        assert(!posts(result));
    }
    /// State-table Queued/person removal: absent snapshot holds, verified resume releases.
    fn resume_witness() {
        let absent = human_hold(false, false);
        let unresolved = human_hold(true, true);
        let resumed = human_hold(true, false);
        assert(absent && unresolved && !resumed);
        let held = human_hold(true, false);
        let read = CommandRead { head: Head::Current, open: true, merged: false,
            draft: false, held, paused: false, waiting: false, eligible: false, now_ms: 0 };
        let result = dispatch(None, read);
        assert(posts(result));
    }
    }
}

#[cfg(verus_keep_ghost)]
mod proof_callers {
    use super::*;
    verus! {
    /// #608/#760: waiting distinguishes eligibility gating, not the shared budget.
    /// All recovery states, all Boolean guards, and full u64 clocks are covered.
    fn same_protocol(recovery: Option<Recovery>, read: CommandRead)
        requires read.eligible,
    {
        let waiting = CommandRead { waiting: true, ..read };
        let legacy = CommandRead { waiting: false, ..read };
        let a = dispatch(recovery, waiting);
        let b = dispatch(recovery, legacy);
        assert(a == b);
        if matches!(a.second, Some(Effect::Write(Write::Command))) {
            assert(a.first == Some(Effect::SaveRecovery));
            assert(ready(a.recovery).command == Command::Sending);
        }
    }
    /// #608: the legacy path does not acquire a new repaired-head eligibility gate.
    fn legacy_witness() {
        let legacy = CommandRead { head: Head::Current, open: true, merged: false,
            draft: false, held: false, paused: false, waiting: false, eligible: false, now_ms: 0 };
        let old = dispatch(None, legacy);
        assert(posts(old));
        let waiting = CommandRead { waiting: true, ..legacy };
        let repaired = dispatch(None, waiting);
        assert(!posts(repaired));
        assert(repaired.first == Some(Effect::SaveRecovery));
    }
    }
}
