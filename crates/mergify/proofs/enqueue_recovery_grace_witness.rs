use crate::enqueue::*;
use crate::enqueue_helpers::*;
use crate::Head;
use vstd::prelude::*;

verus! {
/// Full-domain composition of the original Kani proof: independent arbitrary
/// initial and reread Options, saturating elapsed time, then one-budget replay.
fn protocol(recovery: Option<Recovery>, reread: Option<Recovery>, eligible: bool, now_ms: u64) {
    let state = EnqueueState { intent: None, confirmed: false, recovery, rerun: None };
    let (next, effects) = decide(state, QueueEvent::RecoveryTick { eligible, now_ms });
    let granted = effects.len() > 1 && effects[1] == Effect::Read(Need::Command);
    let read = CommandRead { head: Head::Current, open: true, merged: false, draft: false,
        held: false, paused: false, waiting: true, eligible, now_ms };
    let mut wrote = false;
    if granted {
        let refreshed = EnqueueState { recovery: reread, ..next };
        let (sent, dispatch) = decide(refreshed, QueueEvent::DispatchCommand(read));
        wrote = dispatch.len() > 1 && dispatch[1] == Effect::Write(Write::Command);
        assert(wrote == (ready(reread) && eligible));
        if wrote {
            let (_, again) = decide(sent, QueueEvent::DispatchCommand(read));
            assert(no_write(again@));
        }
    }
    assert(wrote == (ready(reread) && ready(recovery) && eligible && match recovery {
        Some(Recovery { eligible_since_ms: Some(since), .. }) => elapsed(now_ms, since) >= 300000,
        _ => false,
    }));
}

/// All four covers, plus an explicit disappearing-timer reread from #760.
fn covers() {
    let state = EnqueueState { intent: None, confirmed: false, recovery: None, rerun: None };
    let (running, wait) = decide(state, QueueEvent::RecoveryTick { eligible: true, now_ms: 0 });
    assert(running.recovery.unwrap().eligible_since_ms == Some(0));
    assert(wait@ == Seq::empty().push(Effect::SaveRecovery).push(Effect::Complete(Complete::Pending)));
    let (mature, grant) = decide(running, QueueEvent::RecoveryTick { eligible: true, now_ms: 300000 });
    assert(grant@ == Seq::empty().push(Effect::SaveRecovery).push(Effect::Read(Need::Command)));
    let read = CommandRead { head: Head::Current, open: true, merged: false, draft: false,
        held: false, paused: false, waiting: true, eligible: true, now_ms: 300000 };
    let refreshed = EnqueueState { recovery: None, ..mature };
    let (sent, write) = decide(refreshed, QueueEvent::DispatchCommand(read));
    assert(write@ == Seq::empty().push(Effect::SaveRecovery).push(Effect::Write(Write::Command)));
    assert(sent.recovery.unwrap().command == Command::Sending);
    let (_, lost) = decide(sent, QueueEvent::RecoveryTick { eligible: true, now_ms: 600000 });
    assert(no_write(lost@) && lost@ == seq![Effect::Complete(Complete::Pending)]);
    let (reset, _) = decide(running, QueueEvent::RecoveryTick { eligible: false, now_ms: 123 });
    assert(reset.recovery == Some(Recovery { command: Command::Ready, eligible_since_ms: None }));
}
}
