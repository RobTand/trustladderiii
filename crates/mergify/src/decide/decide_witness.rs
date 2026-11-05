//! #743 M1b: executable worlds for all 33 original Kani covers.
use super::check::Status;
use super::facts::*;
use super::observe::QueueState;
use super::translate::admit;
use super::translate::*;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// Q52/#743: the original node constructor spans all four phases.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == (NodeView { phase, merged, enqueue_pending: pending }),))]
fn node(phase: Phase, merged: bool, pending: bool) -> NodeView {
    NodeView {
        phase,
        merged,
        enqueue_pending: pending,
    }
}

/// Q52: all eight original covers, with the original three-suite bound.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn q52_covers() -> bool {
    let publishing = node(Phase::Publishing, false, true);
    let queued = node(Phase::Queued, false, false);
    route(Kind::CheckMissing, publishing, true, true) == Route::MissingIncident
        && route(Kind::CheckMissing, publishing, true, false) == Route::Skip
        && route(Kind::CheckMissing, queued, true, true) == Route::Skip
        && route(Kind::QueueBlocked, queued, true, true) == Route::BlockedIncident
        && required_missing(false, true, false)
        && !required_missing(false, false, false)
        && suites_completed(&[Status::Completed, Status::Completed, Status::Completed])
        && !suites_completed(&[])
}

/// #743/K-translate-closed: late merge, duplicate merge and dropped failure.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn closed_covers() -> bool {
    let closed = node(Phase::Closed, false, false);
    admit(
        Kind::Merged {
            checks_passed: false,
        },
        closed,
    ) && !admit(
        Kind::Merged {
            checks_passed: true,
        },
        node(Phase::Closed, true, false),
    ) && !admit(
        Kind::CiFailed {
            head_check: true,
            infra: false,
        },
        closed,
    )
}

/// #743/K-head-ci-consume: all four queued/enqueuing/waiting covers.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn head_ci_covers() -> bool {
    let kind = Kind::CiFailed {
        head_check: true,
        infra: false,
    };
    admit(kind, node(Phase::Queued, false, false))
        && admit(kind, node(Phase::Publishing, false, true))
        && !admit(kind, node(Phase::Publishing, false, false))
        && !admit(kind, node(Phase::Other, false, true))
}

/// #743/K-translate-route: all five stale-attempt and payload covers.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn route_covers() -> bool {
    let n = node(Phase::Queued, false, false);
    route(
        Kind::CiFailed {
            head_check: false,
            infra: true,
        },
        n,
        false,
        false,
    ) == Route::Record
        && route(Kind::Conflict, n, false, false) == Route::Record
        && route(Kind::DequeuedByCommand, n, false, false) == Route::Record
        && route(
            Kind::CiFailed {
                head_check: false,
                infra: true,
            },
            n,
            true,
            false,
        ) == Route::Kernel(KernelFact::CiFailed { infra: true })
        && route(Kind::DequeuedByCommand, n, true, false)
            == Route::Kernel(KernelFact::PrDequeuedByPerson)
}

/// #743/K-ci-classify: repeated overrides both, outside overrides upstream, upstream stands.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn ci_covers() -> bool {
    let repeated = classify_ci(true, true, true);
    let outside = classify_ci(false, true, false);
    let upstream = classify_ci(true, false, false);
    !repeated.0
        && repeated.1 == Decision::RepeatedTest
        && outside.0
        && outside.1 == Decision::OutsideChange
        && upstream.0
        && upstream.1 == Decision::UpstreamClassification
}

/// #743/K-person-facts: original event order, bot suppression and later-merge suppression.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn person_covers() -> bool {
    decide_proof! { reveal_with_fuel(person_sequence, 4); }
    let dequeue = IssueEvent {
        id: 2,
        actor: Actor::Person,
        kind: EventKind::Unlabeled { queue_label: true },
    };
    let close = IssueEvent {
        id: 1,
        actor: Actor::Person,
        kind: EventKind::Closed,
    };
    let bot = IssueEvent {
        id: 3,
        actor: Actor::Bot,
        kind: EventKind::Closed,
    };
    let merge = IssueEvent {
        id: 4,
        actor: Actor::Missing,
        kind: EventKind::Merged,
    };
    let events = [dequeue, close];
    let bots = [bot];
    let merged = [close, merge];
    let mut both = person_facts(&events, false);
    let mut ignored = person_facts(&bots, false);
    let mut suppressed = person_facts(&merged, false);
    both.next_fact() == Some(PersonFact::Dequeued { event: 2 })
        && both.next_fact() == Some(PersonFact::Closed { event: 1 })
        && both.next_fact().is_none()
        && ignored.next_fact().is_none()
        && suppressed.next_fact().is_none()
}

/// #743/K-facts-outcome: all seven original covers, including callback evaluation.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn outcome_covers() -> bool {
    let unfinished = Some(QueueCheck {
        completed: false,
        concluded: false,
        merged_title: true,
    });
    entry(false, QueueState::Waiting, false, true) == Entry::PushByOther
        && entry(false, QueueState::Waiting, true, true) == Entry::Read { head_checks: true }
        && entry(false, QueueState::Merged, false, true) == Entry::Read { head_checks: false }
        && merged_plan(true, unfinished, true) == MergedPlan::Wait
        && merged_plan(false, unfinished, true) == MergedPlan::ViaQueue
        && queue_outcome(
            QueueState::Dequeued,
            Some(7u8),
            Some(false),
            decide_infra!(unread false),
        ) == Some(Outcome::CiFailed {
            check: 7,
            infra: false,
        })
        && queue_outcome(QueueState::FailedCi, Some(8u8), None, decide_infra!(true))
            == Some(Outcome::CiFailed {
                check: 8,
                infra: true,
            })
}

#[test]
fn original_m1b_covers() {
    assert!(q52_covers());
    assert!(closed_covers());
    assert!(head_ci_covers());
    assert!(route_covers());
    assert!(ci_covers());
    assert!(person_covers());
    assert!(outcome_covers());
}
