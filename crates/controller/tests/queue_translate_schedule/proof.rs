#[path = "../../../mergify/src/decide/translate.rs"]
mod translate;
use vstd::prelude::*;
use translate::*;
use translate::admit as queue_admit;

verus! {
/// Closed row: only an unrecorded late merge is admitted. Unbounded finite state.
fn translate_closed(kind: Kind, merged: bool, enqueue_pending: bool) {
    let node = NodeView { phase: Phase::Closed, merged, enqueue_pending };
    let accepted = queue_admit(kind,node);
    assert(accepted == (matches!(kind, Kind::Merged { .. }) && !merged));
    let witness = NodeView { phase: Phase::Closed, merged:false, enqueue_pending:false };
    let late = queue_admit(Kind::Merged { checks_passed:true },witness);
    let conflict = queue_admit(Kind::Conflict,witness);
    let duplicate = queue_admit(Kind::Merged { checks_passed:true },NodeView { phase:Phase::Closed,merged:true,enqueue_pending:false });
    assert(late && !conflict && !duplicate);
}
/// Q47 Publishing/Queued: a head-CI fact waits for a live enqueue. Unbounded.
fn head_ci_consume(node: NodeView, infra: bool) {
    let accepted = queue_admit(Kind::CiFailed { head_check:true,infra },node);
    assert(accepted == (node.phase == Phase::Queued || (node.phase == Phase::Publishing && node.enqueue_pending)));
    let queued = queue_admit(Kind::CiFailed { head_check:true,infra:false },NodeView { phase:Phase::Queued,merged:false,enqueue_pending:false });
    let enqueue = queue_admit(Kind::CiFailed { head_check:true,infra:false },NodeView { phase:Phase::Publishing,merged:false,enqueue_pending:true });
    let idle = queue_admit(Kind::CiFailed { head_check:true,infra:false },NodeView { phase:Phase::Publishing,merged:false,enqueue_pending:false });
    assert(queued && enqueue && !idle);
}
/// Q52/#447/#373: every setup row stays outside the kernel. Unbounded.
fn q52_no_kernel(kind: Kind, node: NodeView, head_current: bool, approved_head: bool)
    requires matches!(kind, Kind::ChecksUnavailable | Kind::CheckMissing | Kind::QueueBlocked | Kind::CheckReported),
{
    let result = route(kind,node,head_current,approved_head);
    assert(!matches!(result, Route::Kernel(_)));
    let witness = NodeView { phase:Phase::Publishing, merged:false,enqueue_pending:true };
    let setup = route(Kind::ChecksUnavailable,witness,true,true);
    let missing = route(Kind::CheckMissing,witness,true,true);
    let blocked = route(Kind::QueueBlocked,witness,true,true);
    let clear = route(Kind::CheckReported,witness,true,true);
    assert(setup == Route::SetupIncident && missing == Route::MissingIncident && blocked == Route::BlockedIncident && clear == Route::ClearIncident);
}
/// Q50 and Q58: override only when outside or repeated; preserve upstream otherwise.
/// Total over all 8 Boolean inputs; no history-length bound in the decision.
fn ci_classify(upstream: bool, outside: bool, repeated: bool) {
    let (infra,decision) = classify_ci(upstream,outside,repeated);
    assert(infra == (if outside || repeated { outside && !repeated } else { upstream }));
    assert(decision == (if repeated { Decision::RepeatedTest } else if outside { Decision::OutsideChange } else { Decision::UpstreamClassification }));
    let (repeat,repeat_reason) = classify_ci(true,true,true);
    assert(!repeat && repeat_reason == Decision::RepeatedTest);
    let (external,external_reason) = classify_ci(false,true,false);
    assert(external && external_reason == Decision::OutsideChange);
    let (infrastructure,upstream_reason) = classify_ci(true,false,false);
    assert(infrastructure && upstream_reason == Decision::UpstreamClassification);
    let (code,code_reason) = classify_ci(false,false,false);
    assert(!code && code_reason == Decision::UpstreamClassification);
}
/// #373/Q52: recorded or inboxed identities cannot be admitted twice.
/// Inductive decision over arbitrary identities; revision is any i64.
fn intake_once(recorded: bool, inboxed: bool, setup: bool, revision: i64) {
    let duplicate = intake_duplicate(recorded,inboxed);
    assert(duplicate == (recorded || inboxed));
    let recorded = intake_duplicate(true,false);
    let inboxed = intake_duplicate(false,true);
    let fresh = intake_duplicate(false,false);
    assert(recorded && inboxed && !fresh);
    let scoped = intake_revision(setup,revision);
    assert(scoped == (if setup { Some(revision) } else { None }));
    let setup_first = intake_revision(true,0);
    let setup_next = intake_revision(true,1);
    let ordinary_first = intake_revision(false,0);
    let ordinary_next = intake_revision(false,1);
    assert(setup_first != setup_next && ordinary_first == ordinary_next);
}
/// Q52/#447: opening records evidence, reporting clears without a receipt.
/// No sequence bound; the incident identity is supplied by the unchanged shell.
fn incident_pairing(node: NodeView, head_current: bool, approved_head: bool) {
    let missing = route(Kind::CheckMissing,node,head_current,approved_head);
    assert(matches!(missing,Route::MissingIncident | Route::Skip));
    let record_missing = record_incident(missing);
    assert(record_missing == matches!(missing,Route::MissingIncident));
    let clear = route(Kind::CheckReported,node,head_current,approved_head);
    let record_clear = record_incident(clear);
    assert(clear == Route::ClearIncident && !record_clear);
    let setup_record = record_incident(Route::SetupIncident);
    let missing_record = record_incident(Route::MissingIncident);
    let blocked_record = record_incident(Route::BlockedIncident);
    let skip_record = record_incident(Route::Skip);
    assert(setup_record && missing_record && blocked_record && !skip_record);
}
}
fn main() {}
