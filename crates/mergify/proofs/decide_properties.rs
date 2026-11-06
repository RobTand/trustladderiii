//! #743 M1b: every assertion of the original harnesses, without list bounds.
use vstd::prelude::*;
use super::facts::*;
use super::check::Status;
use super::observe::QueueState;
use super::translate::*;
use super::translate::admit;

verus! {
/// Q52: all four setup routes avoid the kernel; missing checks and suites are exact.
fn q52(reported: bool, expects_app: bool, statuses: bool, suites: &[Status], node: NodeView, current: bool, approved: bool) {
    let missing = required_missing(reported, expects_app, statuses);
    assert(!(reported && missing));
    assert(missing == (!reported && (expects_app || statuses)));
    let completed = suites_completed(suites);
    assert(completed == (suites@.len() > 0 && forall|i: int| 0 <= i < suites@.len() ==> suites@[i] == Status::Completed));
    let unavailable = route(Kind::ChecksUnavailable, node, current, approved);
    let absent = route(Kind::CheckMissing, node, current, approved);
    let blocked = route(Kind::QueueBlocked, node, current, approved);
    let report = route(Kind::CheckReported, node, current, approved);
    assert(unavailable == Route::SetupIncident);
    assert(absent == if node.phase == Phase::Publishing && node.enqueue_pending && approved { Route::MissingIncident } else { Route::Skip });
    assert(blocked == if (node.phase == Phase::Queued || node.phase == Phase::Publishing && node.enqueue_pending) && approved { Route::BlockedIncident } else { Route::Skip });
    assert(report == Route::ClearIncident);
    assert(!matches!(unavailable, Route::Kernel(_)) && !matches!(absent, Route::Kernel(_)) && !matches!(blocked, Route::Kernel(_)) && !matches!(report, Route::Kernel(_)));
}
/// #743/K-translate-closed: only a new merge is admitted and routed with its payload.
fn closed(kind: Kind, node: NodeView, current: bool, approved: bool)
    requires node.phase == Phase::Closed,
{
    let admitted = admit(kind, node);
    assert(admitted == (matches!(kind, Kind::Merged { .. }) && !node.merged));
    if admitted {
        let result = route(kind, node, current, approved);
        assert(matches!(result, Route::Kernel(KernelFact::PrMerged { .. })));
    }
}
/// #743/K-head-ci-consume: open nodes gate only head-check failures.
fn head_ci(kind: Kind, node: NodeView)
    requires node.phase != Phase::Closed,
{
    let admitted = admit(kind, node);
    assert(admitted == if matches!(kind, Kind::CiFailed { head_check: true, .. }) { node.phase == Phase::Queued || node.phase == Phase::Publishing && node.enqueue_pending } else { true });
}
/// #743/K-translate-route: independent attempt-kind oracle and exact payload mapping.
fn routed(kind: Kind, node: NodeView, current: bool, approved: bool) {
    let result = route(kind, node, current, approved);
    let stale = matches!(kind, Kind::CiFailed { .. } | Kind::Conflict | Kind::DequeuedByCommand) && !current;
    assert((result == Route::Record) == stale);
    if !stale {
        let expected = match kind {
            Kind::Merged { checks_passed } => Some(KernelFact::PrMerged { checks_passed }),
            Kind::CiFailed { infra, .. } => Some(KernelFact::CiFailed { infra }),
            Kind::Conflict => Some(KernelFact::Conflict),
            Kind::DequeuedByPerson | Kind::DequeuedByCommand => Some(KernelFact::PrDequeuedByPerson),
            Kind::ClosedByPerson => Some(KernelFact::PrClosedByPerson),
            Kind::PushByOther => Some(KernelFact::PushByOther),
            _ => None,
        };
        match expected {
            Some(fact) => { assert(result == Route::Kernel(fact)); },
            None => {
                assert(matches!(kind, Kind::ChecksUnavailable | Kind::CheckMissing | Kind::QueueBlocked | Kind::CheckReported));
                assert(!matches!(result, Route::Kernel(_) | Route::Record));
            },
        }
    }
}
/// #743/K-ci-classify: rule priority, infrastructure decision and its name are exact.
fn classified(upstream: bool, outside: bool, repeated: bool) {
    let (infra, decision) = classify_ci(upstream, outside, repeated);
    assert(!(repeated && infra));
    assert(outside && !repeated ==> infra);
    assert(!outside && !repeated ==> infra == upstream);
    assert(decision == if repeated { Decision::RepeatedTest } else if outside { Decision::OutsideChange } else { Decision::UpstreamClassification });
}
/// #743/K-person-facts: structural induction bounds the exact ordered output by its events.
proof fn person_bound(events: Seq<IssueEvent>, start: int)
    requires 0 <= start <= events.len(),
    ensures person_sequence(events, start).len() <= events.len() - start,
    decreases events.len() - start,
{
    reveal(person_sequence);
    if start < events.len() { person_bound(events, start + 1); }
}
/// #743/K-person-facts: constructor and cursor represent every output, including merge suppression.
fn person_total(events: &[IssueEvent], merged: bool) {
    let iter = person_facts(events, merged);
    proof {
        person_bound(events@, 0);
        assert(iter.remaining().len() <= events@.len());
        if merged || exists|i: int| 0 <= i < events@.len() && events@[i].kind == EventKind::Merged {
            assert(iter.remaining() == Seq::<PersonFact>::empty());
        } else {
            assert(iter.remaining() == person_sequence(events@, 0));
        }
    }
}
/// #743/K-facts-outcome: all entry and merge-plan assertions hold over the full typed domain.
fn outcome_entry(merge_queue: bool, state: QueueState, expected: bool, open: bool, check: Option<QueueCheck>, commit: bool) {
    let read = entry(merge_queue, state, expected, open);
    assert((read == Entry::Ignore) == merge_queue);
    if read == Entry::PushByOther { assert(state != QueueState::Merged && !expected); }
    if !merge_queue && (state == QueueState::Merged || expected) {
        assert(read == Entry::Read { head_checks: open && matches!(state, QueueState::Waiting | QueueState::Unknown | QueueState::FailedCi | QueueState::Dequeued) });
    }
    let plan = merged_plan(expected, check, commit);
    let unfinished = match check { Some(c) => !c.completed || !c.concluded, None => false };
    assert((plan == MergedPlan::Wait) == (expected && unfinished));
    if plan != MergedPlan::Wait {
        assert(plan == if match check { Some(c) => c.merged_title, None => false } { MergedPlan::ViaQueue } else if commit { MergedPlan::MergeCommit } else { MergedPlan::NotPassed });
    }
}
/// #743/K-facts-outcome: original opaque-byte payload and head/report priority.
fn outcome_queue(state: QueueState, check: Option<u8>, head: Option<bool>, report: bool) {
    let result = queue_outcome(state, check, head, decide_infra!(report));
    assert(result == outcome_result(state, check, head, report));
}
}
