//! Q27 approval grants and repair provenance use the production protocol.
#[path = "../src/step.rs"] mod delivery;
use delivery::*;
#[path = "../src/upstream.rs"] mod upstream;
use upstream::*;
use vstd::prelude::*;
verus! {
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
    let checked_3=apply_repair(other,restarted);
    assert(checked_3 == other);
    if copy.exists { assert(late.conclusion == conclusion && late.output == output); }
}

fn history_predecessor_cannot_issue(v: DeliveryView, evidence: Evidence)
    requires v.state == State::Intended, v.approval == (ApprovalGate::Prior { predecessor: true }),
        v.fence == Fence::Live, !v.waits.outage,
{
    let first = action(v, Input::Start);
    if let Action::ReadHistory(pending) = first {
        let second = action(v, Input::History { pending, evidence });
        if let Action::CommitSent { spend_grant, then } = second {
            assert(spend_grant);
            let sent = DeliveryView { state: State::Sent, ..v };
            let next = action(sent, Input::SentCommitted(then));
            assert(matches!(next, Action::RecordAmbiguity { stop: true }));
        } else { assert(false); }
    } else { assert(false); }
}

fn pending_history_cannot_issue(v: DeliveryView)
    requires v.state == State::Intended, v.approval == (ApprovalGate::Prior { predecessor: false }),
        v.fence == Fence::Live, !v.waits.outage,
{
    let first = action(v, Input::Start);
    if let Action::ReadHistory(pending) = first {
        let second = action(v, Input::History { pending, evidence: Evidence::Pending });
        if let Action::CommitSent { spend_grant, then } = second {
            assert(spend_grant);
            let next = action(DeliveryView { state: State::Sent, ..v }, Input::SentCommitted(then));
            assert(matches!(next, Action::RecordAmbiguity { stop: true }));
        } else { assert(false); }
    } else { assert(false); }
}

fn grant_spent_at_sent(v: DeliveryView)
    requires v.state == State::Intended, v.approval == ApprovalGate::Authorized,
        v.fence == Fence::Live, !v.waits.outage,
{
    let next = action(v, Input::Start);
    assert(matches!(next, Action::CommitSent { spend_grant: true, .. }));
    // The interpreter commits Intended->Sent and grant deletion atomically.
    // A restart has no continuation, so Start from durable Sent cannot issue.
    let restarted = action(DeliveryView { state: State::Sent, approval: (ApprovalGate::Prior { predecessor: false }), ..v }, Input::Start);
    assert(!matches!(restarted, Action::CommitSent { .. } | Action::Execute(_)));
}

fn clear_requires_matching_active_approval(v: DeliveryView, key: &[u8], matching: bool) {
    let next = action(v, Input::ClearApproval { key, matching_open_incident: matching });
    if let Action::AuthorizeRetry = next {
        assert(v.key@ == key@ && matching && v.approval != ApprovalGate::Ordinary);
        assert(matches!(v.state, State::Sent | State::Uncertain));
    }
}

fn replacement_provenance(v: DeliveryView, ack: Ack, successor: bool) {
    let next = action(v, Input::Consume { key: v.key, ack, kernel_ack: ack, approval_successor: successor });
    if let Action::Commit(p) = next {
        if p.authorize_successor {
            assert(successor && ack == Ack::Retryable && v.kind == DeliveryKind::KernelEffect);
            assert(v.approval != ApprovalGate::Ordinary && p.retire_retry && p.remove_watch);
        }
    }
}

fn final_news_recovery(v: DeliveryView, copies: Option<u64>, dispatches: u64)
    requires v.approval != ApprovalGate::Ordinary,
        matches!(v.state, State::Confirmed | State::Failed | State::Cancelled),
{
    let next = action(v, Input::CheckedApprovalNews { key: v.key });
    assert(matches!(next, Action::ScheduleRepair));
    let finished = action(v, Input::RepairDone { copies, dispatches });
    assert(matches!(finished, Action::FinishRepair { clear_unresolved } if clear_unresolved == match copies { Some(c) => c >= dispatches, None => false }));
}

/// A delayed POST can land after confirmation; all copies still converge to
/// the same conclusion and output. News schedules repair even for a final row.
fn approval_late_witness(v: DeliveryView)
    requires v.state == State::Confirmed, v.approval != ApprovalGate::Ordinary,
{
    let initial = Upstream { copies: 0, incorrect: 0, outstanding: 0, repair_debt: 0, found: false };
    let old = lose_response(issue(initial));
    let retry = issue(restart(old));
    let first = land(retry, Class::Approval, true, OnceLaws { atomic_original_key_dedup: false, stable_operation_identity: false });
    let zombie = land(restart(first), Class::Approval, false, OnceLaws { atomic_original_key_dedup: false, stable_operation_identity: false });
    let news = action(v, Input::CheckedApprovalNews { key: v.key });
    assert(matches!(news, Action::ScheduleRepair));
    let repaired = approval_converges(zombie);
    assert(repaired.copies == 2 && repaired.incorrect == 0);
    let finished = action(v, Input::RepairDone { copies: Some(repaired.copies), dispatches: 2 });
    assert(matches!(finished, Action::FinishRepair { clear_unresolved: true }));
}
}
fn main() {}
