//! State-table messages: at least once, then delete extra matching copies.
#[path = "../src/upstream.rs"] mod upstream;
use upstream::*;
#[path = "../src/step.rs"] mod delivery;
use delivery::{State,Ack,completion};
use vstd::prelude::*;
verus! {
/// State-table messages: one complete successful reconciliation after quiescence
/// retains exactly one copy. Each DELETE is separate and may precede a crash.
/// All finite u64 copy counts, no trace bound and no unrolling bound.
fn deduplicate_after_quiescence(before: Upstream) -> (out: Upstream)
    requires before.copies > 0, before.incorrect == 0, before.outstanding == 0,
    ensures out.copies == 1, out.found, out.outstanding == 0,
{
    let mut current = read(before,Class::Deduplicated);
    while current.repair_debt > 0
        invariant current.copies == current.repair_debt + 1,
            current.outstanding == 0, current.incorrect == 0, current.found,
        decreases current.repair_debt,
    {
        let target = CopyState {id: current.repair_debt, exists:true, conclusion:1, output:1};
        let flight = issue_repair(RepairRequest {target:target.id,kind:RepairKind::Delete,conclusion:0,output:0});
        current = advance_repair_cursor(current);
        let applied = apply_counted_repair(current,target,flight);
        current = applied.upstream;
    }
    current
}
/// State-table messages / F3: an empty look, retry, partial cleanup, restart,
/// then a zombie copy is handled by the next full matching-marker cleanup.
fn dedup_late_witness() {
    let initial = Upstream {copies:0,incorrect:0,outstanding:0,repair_debt:0,found:false};
    let old = issue(initial);
    let absent = read(lose_response(old),Class::Deduplicated);
    let retry = issue(restart(absent));
    let first = land(retry,Class::Deduplicated,true,OnceLaws {atomic_original_key_dedup:false,stable_operation_identity:false});
    let confirmed_read = read(first,Class::Deduplicated);
    assert(confirmed_read.found && confirmed_read.copies >= 1);
    let zombie = land(restart(confirmed_read),Class::Deduplicated,true,OnceLaws {atomic_original_key_dedup:false,stable_operation_identity:false});
    assert(zombie.copies == 2);
    let fixed = deduplicate_after_quiescence(zombie);
    assert(fixed.copies == 1);
}
/// State-table Deduplicated: an upstream Found enters the REAL completion plan.
/// In the per-key model, confirmation implies at least one upstream copy.
fn receipt_uses_real_completion(before: Upstream) -> (out: State)
    requires before.incorrect <= before.copies,
    ensures (out == State::Confirmed) == (before.copies > 0),
{
    let observed=read(before,Class::Deduplicated);
    if observed.found {completion(State::Uncertain,Ack::Confirmed).to}
    else {State::Uncertain}
}

/// R3/state-table messages: a timed-out DELETE retains its immutable target ID
/// across restart, even if a new reconciliation has a different list cursor.
/// Applying both requests in either order never deletes the keeper/another key.
/// Universal IDs and repeat steps give unbounded finite-schedule induction.
fn delayed_delete_is_idempotent(copy:CopyState, keeper:CopyState)
    requires keeper.id != copy.id,
{
    let request=RepairRequest {target:copy.id,kind:RepairKind::Delete,conclusion:0,output:0};
    let old=lose_repair_response(issue_repair(request));
    let restarted=restart_repair(old);
    assert(restarted.issued && !restarted.cursor);
    let current=apply_repair(copy,issue_repair(request));
    let late=apply_repair(current,restarted);
    assert(!late.exists && late == current);
    assert(apply_repair(late,restarted) == late);
    assert(apply_repair(keeper,restarted) == keeper);
}
/// Count projection cannot delete an extra copy twice when the old request lands
/// after a newer controller has already removed its target.
fn counted_late_delete_witness() {
    let copies=Upstream {copies:2,incorrect:0,outstanding:0,repair_debt:1,found:true};
    let target=CopyState {id:9,exists:true,conclusion:0,output:0};
    let request=RepairRequest {target:9,kind:RepairKind::Delete,conclusion:0,output:0};
    let old=restart_repair(lose_repair_response(issue_repair(request)));
    let restarted=restart(copies);
    assert(restarted.repair_debt == 0 && old.issued);
    let current=apply_counted_repair(restarted,target,issue_repair(request));
    let late=apply_counted_repair(current.upstream,current.copy,old);
    assert(late.upstream.copies == 1);
    assert(!late.copy.exists);
}

}
fn main() {}
