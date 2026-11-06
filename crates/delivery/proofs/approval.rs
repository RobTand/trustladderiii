//! State-machine Q27 :961: all matching approval copies converge identically.
#[path = "../src/step.rs"] mod step;
pub use step::*;
#[path = "../src/mirror.rs"] mod mirror;
#[path = "../src/upstream.rs"] mod upstream;
use upstream::*;
use vstd::prelude::*;
verus! {
/// Q27: after quiescence, a successful full reconciliation PATCHes every visible
/// noncanonical conclusion/output to the original operation's canonical value.
/// Incorrect counts encode BOTH conclusion and output, not conclusion alone.
/// All u64 copy counts; unbounded finite schedules, partial repairs and restarts.
fn approval_converges(before: Upstream) -> (out: Upstream)
    requires before.incorrect <= before.copies, before.outstanding == 0,
    ensures out.incorrect == 0, out.copies == before.copies,
        out.outstanding == 0, out.found == (before.copies > 0),
{
    let mut current = read(before,Class::Approval);
    while current.repair_debt > 0
        invariant current.incorrect == current.repair_debt,
            current.copies == before.copies, current.outstanding == 0,
            current.found == (before.copies > 0),
        decreases current.repair_debt,
    {
        let target = CopyState {id: current.repair_debt, exists:true, conclusion:1, output:1};
        let flight = issue_repair(RepairRequest {target:target.id,kind:RepairKind::Patch,conclusion:0,output:0});
        current = advance_repair_cursor(current);
        let applied = apply_counted_repair(current,target,flight);
        current = applied.upstream;
    }
    current
}
/// Q27: for ANY reachable durable approval snapshot, an unknown dispatched
/// attempt requires an unconsumed authorization. Absence alone cannot grant it.
/// One arbitrary state step; inductive under clear/send/ambiguity/news/repair.
fn grant_is_required(before: mirror::Approval, predecessor: bool, sidecar: mirror::Sidecar)
    requires before.grant == mirror::Grant::None,
        matches!(sidecar,mirror::Sidecar::Dispatching | mirror::Sidecar::Uncertain | mirror::Sidecar::Unknown),
{
    let evidence=mirror::approval_evidence(sidecar,false);
    assert(evidence == mirror::Read::Pending);
    let decision=mirror::approval_decision(before,true,predecessor,evidence);
    assert(decision != mirror::Action::Send);
}
/// Q27: human clear produces Intended+grant+safe lineage in one commit. Sent
/// atomically spends it. No second retry can use this grant, even after restart,
/// ambiguity, or an observation early-return before HTTP. No bounded traces.
fn grant_spent_exactly_once(before: mirror::Approval)
    requires before.incident_open, matches!(before.state,State::Sent | State::Uncertain),
{
    let grant=mirror::clear_approval(before);
    assert(grant.grant == mirror::Grant::HumanClear && grant.lineage_safe);
    assert(grant.state == State::Intended);
    assert(mirror::approval_decision(grant,true,false,mirror::Read::Pending) == mirror::Action::Send);
    let sent=mirror::send_approval(grant);
    assert(sent.state == State::Sent && sent.grant == mirror::Grant::None);
    let observed=mirror::continue_after_observations(true);
    assert(!observed);
    let uncertain=mirror::Approval {state:State::Uncertain,..sent};
    let incident=mirror::ambiguous_approval(uncertain);
    assert(incident.grant == mirror::Grant::None);
    assert(mirror::approval_decision(incident,true,false,mirror::Read::Pending) != mirror::Action::Send);
}
/// lib.rs:1853-1866: replacement provenance comes from actual Retryable kernel
/// completion, not the empty network read. Old sequence, watch and retirement
/// change in the same input commit that creates the successor authorization.
fn replacement_provenance(state: State)
    requires matches!(state,State::Sent | State::Uncertain),
{
    let input=mirror::CompletionInput {state,queued:false,ack:Ack::Confirmed,
        kernel_effect:true,replacement:false,replacement_grant:false,watch:true,retirement:false};
    let queued=mirror::enqueue_completion(input,Ack::Retryable);
    assert(queued.state == state && queued.queued);
    let consumed=mirror::consume_completion(queued,true,true);
    assert(consumed.state == State::Cancelled && !consumed.watch && consumed.retirement);
    let next=mirror::replacement_approval(consumed);
    assert(next.grant == mirror::Grant::RetryableReplacement && next.lineage_safe);
    let sent=mirror::send_approval(next);
    assert(sent.grant == mirror::Grant::None);
}
/// Q27: a final row remains final while check news durably schedules a repair.
/// Repair retirement only clears UNRESOLVED if checked copies cover all Sent intents.
fn final_news_recovery(before: mirror::Approval, copies:u64, dispatches:u64)
    requires matches!(before.state,State::Confirmed | State::Failed | State::Cancelled),
{
    let news=mirror::approval_news(before);
    assert(news.repair && news.state == before.state && news.grant == before.grant);
    let repaired=mirror::repaired_approval(news,copies,dispatches);
    assert(!repaired.repair && repaired.state == before.state);
    assert(repaired.unresolved == (before.unresolved && copies < dispatches));
}
/// R3/Q27: in-flight PATCH survives restart and can apply after another repair.
/// Arbitrary target, arbitrary conclusion/output, arbitrary number of replays.
fn delayed_patch_is_idempotent(copy:CopyState, conclusion:u64, output:u64, other_id:u64)
    requires other_id != copy.id,
{
    let request=RepairRequest {target:copy.id,kind:RepairKind::Patch,conclusion,output};
    let old=lose_repair_response(issue_repair(request));
    let restarted=restart_repair(old);
    assert(restarted.issued && !restarted.cursor);
    let new=apply_repair(copy,issue_repair(request));
    let late=apply_repair(new,restarted);
    assert(late == new);
    let replay=apply_repair(late,restart_repair(restarted));
    assert(replay == late);
    let other=CopyState {id:other_id,..copy};
    assert(apply_repair(other,restarted) == other);
    if copy.exists { assert(late.conclusion == conclusion && late.output == output); }
}
/// Q27 / F3: an old POST can land after confirmation and restart. Its differing
/// output is repaired by original key, without acknowledging a new-epoch effect.
fn approval_late_witness() {
    let initial=Upstream {copies:0,incorrect:0,outstanding:0,repair_debt:0,found:false};
    let original=mirror::Approval {state:State::Intended,grant:mirror::Grant::None,
        lineage_safe:false,unresolved:false,incident_open:false,repair:false};
    let sent=mirror::send_approval(original);
    let old=issue(initial);
    let delayed=lose_response(old);
    let ambiguous=mirror::ambiguous_approval(sent);
    let grant=mirror::clear_approval(ambiguous);
    assert(mirror::approval_decision(grant,true,false,mirror::Read::Pending) == mirror::Action::Send);
    let retry_sent=mirror::send_approval(grant);
    assert(retry_sent.grant == mirror::Grant::None);
    let retry=issue(restart(delayed));
    let first=land(retry,Class::Approval,true,OnceLaws {atomic_original_key_dedup:false,stable_operation_identity:false});
    let confirmed=mirror::Approval {state:State::Confirmed,..retry_sent};
    let zombie=land(restart(first),Class::Approval,false,OnceLaws {atomic_original_key_dedup:false,stable_operation_identity:false});
    let scheduled=mirror::approval_news(confirmed);
    assert(scheduled.repair);
    let fixed=approval_converges(zombie);
    let finished=mirror::repaired_approval(scheduled,fixed.copies,2);
    assert(fixed.copies == 2 && fixed.incorrect == 0);
    assert(finished.state == State::Confirmed && !finished.repair);
    // A stale PATCH can still land after the repair completed, but sets the
    // identical canonical fields of this original operation.
    let copy=CopyState {id:2,exists:true,conclusion:9,output:8};
    let patch=issue_repair(RepairRequest {target:2,kind:RepairKind::Patch,conclusion:1,output:2});
    let late=restart_repair(lose_repair_response(patch));
    let repaired=apply_repair(copy,patch);
    assert(apply_repair(repaired,late) == repaired);
}
}
fn main() {}
