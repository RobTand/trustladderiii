use crate::enqueue::*;
use crate::enqueue_helpers::*;
use crate::{Head, Status};
use vstd::prelude::*;

verus! {
/// At most one POST on a returned consumed receipt until a result, scoped to
/// the shell's selected run. Universal state/attempt/head/status/writable;
/// Pending preserves the receipt, so any finite number of these two event
/// families preserves the blocking invariant. External receipt erasure is
/// outside this guarantee. Escape: preserved existing behaviour (no rule),
/// base 504f10b0 enqueue.rs:640-649,688-703; Q47 retry context as in the spec.
fn consumed_receipt(state: EnqueueState, saved: Rerun, head: Head, attempt: u64,
    status: Status, writable: Option<bool>) -> (result: (EnqueueState, Effects))
    requires !refused(state), state.rerun == Some(saved), !saved.waiting,
    ensures result.0 == state, no_write(result.1@),
{
    let (pending_state, pending_effects) = decide(state, QueueEvent::RerunPending { head, attempt, status });
    assert(pending_state == state);
    let (next, effects) = decide(pending_state, QueueEvent::RerunRead { head, attempt, status, writable });
    assert(next == state);
    assert(no_write(effects@));
    (next, effects)
}

/// Compose actual dispatch, pending query and arbitrary repeated read. No result
/// is delivered between them. A newer attempt also cannot cause another POST.
fn dispatch_once(state: EnqueueState, attempt: u64, later: u64, head: Head,
    status: Status, writable: Option<bool>)
    requires !refused(state), state.rerun == None,
{
    let (sent, effects) = decide(state, QueueEvent::RerunRead { head: Head::Current,
        attempt, status: Status::Completed, writable: Some(true) });
    assert(effects@ == Seq::empty().push(Effect::SaveRerun).push(Effect::Write(Write::Rerun)));
    assert(sent.intent == state.intent);
    assert(sent.rerun == Some(Rerun { attempt, accepted: false, waiting: false }));
    let (replayed, replay_effects) = consumed_receipt(sent,
        Rerun { attempt, accepted: false, waiting: false }, head, later, status, writable);
    assert(replayed == sent);
    assert(no_write(replay_effects@));
}

/// InProgress enables waiting, but an incomplete run must never be re-POSTed.
/// An Accepted or Unknown response also prevents another POST for all reads.
/// Refused clears the receipt, so a later eligible read may dispatch again.
fn result_budget(state: EnqueueState, receipt: Rerun, result: RerunResult,
    head: Head, attempt: u64, status: Status, writable: Option<bool>)
    requires !refused(state), state.rerun == Some(receipt), !receipt.waiting,
{
    let (done, effects) = decide(state, QueueEvent::RerunDone(result));
    assert(effects@ == seq![Effect::SaveRerun]);
    let (_, replay) = decide(done, QueueEvent::RerunRead { head, attempt, status, writable });
    if result == RerunResult::Accepted || result == RerunResult::Unknown ||
        result == RerunResult::InProgress && status != Status::Completed {
        assert(no_write(replay@));
    }
}

/// Concrete branch witnesses for every read gate and the write-ahead receipt.
fn read_covers() {
    let empty = EnqueueState { intent: None, confirmed: false, recovery: None, rerun: None };
    let (_, stale) = decide(empty, QueueEvent::RerunRead { head: Head::Other,
        attempt: 0, status: Status::Completed, writable: Some(true) });
    assert(stale@ == seq![Effect::Error(Failure::PullChanged)]);
    let (waiting, running) = decide(empty, QueueEvent::RerunRead { head: Head::Current,
        attempt: 0, status: Status::InProgress, writable: Some(true) });
    assert(running@ == Seq::empty().push(Effect::SaveRerun).push(Effect::Error(Failure::RerunFinishing)));
    let (_, read) = decide(waiting, QueueEvent::RerunRead { head: Head::Current,
        attempt: 0, status: Status::Completed, writable: None });
    assert(read@ == seq![Effect::Read(Need::RerunPull)]);
    let (_, closed) = decide(waiting, QueueEvent::RerunRead { head: Head::Current,
        attempt: 0, status: Status::Completed, writable: Some(false) });
    assert(closed@ == seq![Effect::Error(Failure::PullChanged)]);
    let (sent, post) = decide(waiting, QueueEvent::RerunRead { head: Head::Current,
        attempt: 0, status: Status::Completed, writable: Some(true) });
    assert(post@ == Seq::empty().push(Effect::SaveRerun).push(Effect::Write(Write::Rerun)));
    let (_, blocked) = decide(sent, QueueEvent::RerunRead { head: Head::Current,
        attempt: 0, status: Status::Completed, writable: Some(true) });
    assert(blocked@ == seq![Effect::Error(Failure::RerunUnknown)]);
    let (_, advanced) = decide(sent, QueueEvent::RerunRead { head: Head::Current,
        attempt: 1, status: Status::Completed, writable: Some(true) });
    assert(advanced@ == seq![Effect::Continue]);
}

/// All four result classes, with and without an existing receipt. Waiting can
/// resume only after completion; refusal resets rather than consuming budget.
fn result_covers() {
    let sent = EnqueueState { intent: None, confirmed: false, recovery: None,
        rerun: Some(Rerun { attempt: 7, accepted: false, waiting: false }) };
    let empty = EnqueueState { rerun: None, ..sent };
    let (none_a, _) = decide(empty, QueueEvent::RerunDone(RerunResult::Accepted));
    assert(none_a == empty);
    let (none_w, _) = decide(empty, QueueEvent::RerunDone(RerunResult::InProgress));
    assert(none_w == empty);
    let (none_r, _) = decide(empty, QueueEvent::RerunDone(RerunResult::Refused));
    assert(none_r == empty);
    let (none_u, _) = decide(empty, QueueEvent::RerunDone(RerunResult::Unknown));
    assert(none_u == empty);
    let (accepted, a) = decide(sent, QueueEvent::RerunDone(RerunResult::Accepted));
    assert(accepted.rerun == Some(Rerun { attempt: 7, accepted: true, waiting: false }));
    assert(a@ == seq![Effect::SaveRerun]);
    let (unknown, u) = decide(sent, QueueEvent::RerunDone(RerunResult::Unknown));
    assert(unknown == sent); assert(u@ == seq![Effect::SaveRerun]);
    let (waiting, w) = decide(sent, QueueEvent::RerunDone(RerunResult::InProgress));
    assert(waiting.rerun == Some(Rerun { attempt: 7, accepted: false, waiting: true }));
    assert(w@ == seq![Effect::SaveRerun]);
    let (_, resumed) = decide(waiting, QueueEvent::RerunRead { head: Head::Current,
        attempt: 7, status: Status::Completed, writable: Some(true) });
    assert(resumed@ == Seq::empty().push(Effect::SaveRerun).push(Effect::Write(Write::Rerun)));
    let (refused, r) = decide(sent, QueueEvent::RerunDone(RerunResult::Refused));
    assert(refused.rerun == None); assert(r@ == seq![Effect::SaveRerun]);
    let (_, retry) = decide(refused, QueueEvent::RerunRead { head: Head::Current,
        attempt: 7, status: Status::Completed, writable: Some(true) });
    assert(retry@ == Seq::empty().push(Effect::SaveRerun).push(Effect::Write(Write::Rerun)));
}

/// Reach pending true/false, accepted short-circuit and refused-intent gate.
fn pending_covers() {
    let empty = EnqueueState { intent: None, confirmed: false, recovery: None, rerun: None };
    let (_, absent) = decide(empty, QueueEvent::RerunPending { head: Head::Current,
        attempt: 0, status: Status::Completed });
    assert(absent@ == seq![Effect::RerunPending(false)]);
    let saved = EnqueueState { rerun: Some(Rerun { attempt: 1, accepted: false, waiting: true }), ..empty };
    let (_, running) = decide(saved, QueueEvent::RerunPending { head: Head::Current,
        attempt: 1, status: Status::InProgress });
    assert(running@ == seq![Effect::RerunPending(true)]);
    let (_, completed) = decide(saved, QueueEvent::RerunPending { head: Head::Current,
        attempt: 1, status: Status::Completed });
    assert(completed@ == seq![Effect::RerunPending(false)]);
    let (accepted, _) = decide(saved, QueueEvent::RerunDone(RerunResult::Accepted));
    let (_, done) = decide(accepted, QueueEvent::RerunRead { head: Head::Current,
        attempt: 1, status: Status::Completed, writable: Some(true) });
    assert(done@ == seq![Effect::Continue]);
    let refused = EnqueueState { intent: Some(Intent { requeue: Some(Requeue::Refused) }), ..saved };
    let (next, effects) = decide(refused, QueueEvent::RerunDone(RerunResult::Refused));
    assert(next.rerun == refused.rerun);
    assert(effects@ == Seq::empty().push(Effect::SaveRecovery).push(Effect::Complete(Complete::Permanent(Failure::Refused))));
}
}
