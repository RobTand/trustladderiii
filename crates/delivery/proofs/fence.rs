//! Corrected state-table fence contract F1-F3, without an issue-time fence.
#[path = "../src/step.rs"] mod step;
pub use step::*;
#[path = "../src/mirror.rs"] mod mirror;
use mirror::*;
use vstd::prelude::*;
verus! {
/// State-table :755 F1, every u64 epoch pair. Production i64 fence sampling is
/// conformed by the recorded real record.rs shell, including malformed reads.
fn entry_refuses_older_epochs(epoch: u64, fence: u64) {
    let issuer=begin_issuer(State::Sent,epoch);
    let accepted=entry(issuer,fence);
    assert(accepted.admitted == (epoch >= fence));
}
/// State-table :731-734 / real ActiveDeliveries guard F2. While execute blocks,
/// the live Controller is sequential and its own effect remains Sent, checked
/// in the recording executor conformance test. No schedule/unwind bound.
fn no_live_overlap(issuer: Issuer, state: State)
    requires issuer.owns && issuer.in_call ==> state == State::Sent,
{
    let blocked=blocks_epoch_advance(state);
    assert(!blocked && issuer.owns ==> !issuer.in_call);
}
/// F3 witness: sample fence, pause GET, lose ownership, recover the old row,
/// advance epoch, then issue the old POST. It still names a durable Sent attempt.
/// The class proofs compose this issue with nondeterministic late application.
fn zombie_issue_witness() {
    let sent=begin_issuer(State::Sent,1);
    let entry_read=entry(sent,1);
    let zombie=lose_ownership(entry_read);
    let cancelling=dispatch(State::Sent,true,false);
    let old=committed(State::Sent,cancelling,Ack::Confirmed);
    assert(old == State::Cancelling);
    let cancelled=committed(old,Action::Cancel,Ack::Retryable);
    let blocked=blocks_epoch_advance(cancelled);
    assert(!blocked);
    let issued=issue_http(zombie);
    assert(issued.issued && issued.durably_sent && !issued.owns);
    assert(issued.epoch == 1);
}
}
fn main() {}
