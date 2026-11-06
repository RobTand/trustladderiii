//! State-table Converges / late requests :753.
#[path = "../src/step.rs"] mod step;
pub use step::*;
#[path = "../src/mirror.rs"] mod mirror;
#[path = "../src/upstream.rs"] mod upstream;
use upstream::*;
use vstd::prelude::*;
verus! {
/// State-table Converges: after quiescence an ordinary pending reconciliation
/// confirms iff the upstream read finds the desired state. Holds preserve an
/// absent Uncertain effect; they never turn absence into confirmation.
fn reconciliation_agrees_with_quiescent_upstream(before: Upstream, held: bool)
    requires before.outstanding == 0, before.incorrect <= before.copies,
{
    let observed = read(before,Class::Converges);
    let result = if observed.found {mirror::Read::Found} else {mirror::Read::Absent};
    let action = mirror::reconciliation(State::Uncertain,result,held,false,false);
    let after = mirror::committed(State::Uncertain,action,Ack::Confirmed);
    assert((after == State::Confirmed) == (before.copies > 0));
}
/// State-table Effects late-landing paragraph / F3, R1 counterexample. Final
/// keys leave pending_effects. Startup retires their closed-node allowances,
/// so a create landing after teardown has no scheduled recovery. This witness
/// records the missing guarantee, rather than assuming a closed-node look.
/// Bound: one original request, one restart; the pending reconciliation lemma
/// above remains valid. Full post-final Converges is BLOCKED on the owner fix.
fn late_landing_after_final_exposes_missing_recovery() {
    let old=issue(Upstream {copies:0,incorrect:0,outstanding:0,repair_debt:0,found:false});
    let absent=read(lose_response(old),Class::Converges);
    assert(!absent.found);
    let cancelled=mirror::committed(State::Cancelling,mirror::Action::Cancel,Ack::Retryable);
    assert(cancelled == State::Cancelled);
    let checked_1=mirror::recovery(cancelled,false,false);
    assert(checked_1 == mirror::Recovery::None);
    let zombie=land(restart(absent),Class::Converges,true,OnceLaws {atomic_original_key_dedup:false,stable_operation_identity:false});
    let scheduled=mirror::recovery(cancelled,false,false);
    assert(scheduled == mirror::Recovery::None);
    assert(zombie.copies == 1 && zombie.outstanding == 0);
    assert(cancelled == State::Cancelled);
}
/// R1: history commits, input enqueue and consume are distinct crash cuts.
/// Any starting active state and ack, with optional real kernel replacement.
fn durable_completion_boundaries(state:State, ack:Ack, kernel:bool, replacement:bool)
    requires matches!(state,State::Sent | State::Uncertain | State::Cancelling),
{
    let input=mirror::CompletionInput {state,ack,queued:false,kernel_effect:kernel,
        replacement:false,replacement_grant:false,watch:true,retirement:false};
    let queued=mirror::enqueue_completion(input,ack);
    assert(queued.state == state && queued.queued);
    let after=mirror::consume_completion(queued,false,replacement);
    assert(!after.queued);
    if kernel && ack == Ack::Retryable && state != State::Cancelling {
        assert(after.state == State::Cancelled && after.retirement && !after.watch);
        assert(after.replacement == replacement);
    }
    let found=mirror::approval_history(true,false,false,mirror::Read::Found);
    let pending=mirror::approval_history(true,false,false,mirror::Read::Pending);
    assert(found.sent == State::Sent && found.outcome == mirror::Action::Complete);
    assert(pending.sent == State::Sent && pending.outcome == mirror::Action::Ambiguous);
}
}
fn main() {}
