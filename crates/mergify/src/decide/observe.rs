//! Which queue check describes a PR, and what state the queue is in.

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

use super::check::{App, CheckView, Conclusion, Head, Name, Status};

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum QueueState {
    Waiting,
    Queued,
    ChecksRunning,
    Merged,
    Dequeued,
    DequeuedByPerson,
    FailedCi,
    Conflict,
    Unknown,
}

/// GitHub's `mergeable_state`, as far as the queue decisions read it.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Mergeable {
    /// No `mergeable_state` field.
    Missing,
    /// `unknown`: GitHub has not computed it yet.
    Unknown,
    /// `dirty`: the PR conflicts with its base.
    Dirty,
    Other,
}

impl Mergeable {
    pub fn parse(state: Option<&str>) -> Self {
        match state {
            None => Self::Missing,
            Some("unknown") => Self::Unknown,
            Some("dirty") => Self::Dirty,
            Some(_) => Self::Other,
        }
    }
}

/// The parts of a pull request the queue state depends on.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct PullView {
    /// GitHub records a merge time.
    pub merged: bool,
    pub mergeable: Option<bool>,
    pub mergeable_state: Mergeable,
}

/// Only a check from Mergify's app on the PR's current head describes current
/// state. Of those with `name`, the one with the largest ID; on a tie, the
/// last, as `Iterator::max_by_key` chooses.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures selected(checks@, name, checks@.len() as int, result),
))]
pub fn select(checks: &[CheckView], name: Name) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut i = 0;
    decide_while! { (i < checks.len()) [
        invariant i <= checks@.len(), selected(checks@, name, i as int, best),
        decreases checks@.len() - i,
    ] {
        let check = &checks[i];
        if check.head == Head::Current
            && check.name == name
            && check.app == App::Mergify
            && match best { None => true, Some(best) => check.id >= checks[best].id }
        {
            best = Some(i);
        }
        i += 1;
    } }
    best
}

/// A timeout or a runner that never started fails the queue's CI whatever the
/// report says, and `report` is not read. Otherwise the report decides.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    requires matches!(conclusion, Some(Conclusion::TimedOut | Conclusion::StartupFailure)) || report.requires(()),
    ensures if matches!(conclusion, Some(Conclusion::TimedOut | Conclusion::StartupFailure)) {
        result == QueueState::FailedCi
    } else { report.ensures((), result) },
))]
pub fn check_state(
    conclusion: Option<Conclusion>,
    report: impl FnOnce() -> QueueState,
) -> QueueState {
    if matches!(
        conclusion,
        Some(Conclusion::TimedOut | Conclusion::StartupFailure)
    ) {
        QueueState::FailedCi
    } else {
        report()
    }
}

/// GitHub's merge record wins over a queue report, including a delayed one,
/// and the report is then not read. A dequeue on a PR that conflicts with its
/// base is a conflict. `reported` is the selected queue check's state.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    requires pull.merged || report.requires(()),
    ensures (result == QueueState::Merged) == pull.merged,
        !pull.merged ==> exists|r: Option<QueueState>| report.ensures((), r)
            && result == queue_result(pull, r),
))]
pub fn queue_state(pull: PullView, report: impl FnOnce() -> Option<QueueState>) -> QueueState {
    if pull.merged {
        return QueueState::Merged;
    }
    let report_value = report();
    decide_proof! { assert(report.ensures((), report_value)); }
    let reported = report_value.unwrap_or(QueueState::Unknown);
    let result = if reported == QueueState::Merged {
        QueueState::Unknown
    } else if reported == QueueState::Dequeued
        && (pull.mergeable == Some(false) || pull.mergeable_state == Mergeable::Dirty)
    {
        QueueState::Conflict
    } else {
        reported
    };
    decide_proof! {
        assert(result == queue_result(pull, report_value));
        assert(exists|r: Option<QueueState>| report.ensures((), r) && result == queue_result(pull, r));
    }
    result
}

/// A dequeue can't be told from a conflict until GitHub computes mergeability.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == (state == QueueState::Dequeued && pull.mergeable != Some(true)
        && matches!(pull.mergeable_state, Mergeable::Missing | Mergeable::Unknown)),
))]
pub fn mergeability_pending(state: QueueState, pull: PullView) -> bool {
    state == QueueState::Dequeued
        && pull.mergeable != Some(true)
        && matches!(
            pull.mergeable_state,
            Mergeable::Missing | Mergeable::Unknown
        )
}

/// The queue rule's check concluded at or after the selected queue check did,
/// so a dequeue came from the rule rather than from a person.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == rule_result(checks@, queue),
))]
pub fn rule_concluded(checks: &[CheckView], queue: Option<usize>) -> bool {
    let Some(queue) = (match queue {
        Some(i) => checks.get(i),
        None => None,
    }) else {
        return false;
    };
    match select(checks, Name::QueueRule) {
        None => false,
        Some(rule) => {
            let rule = &checks[rule];
            rule.status == Status::Completed
                && rule.conclusion.is_some()
                && match (rule.completed_at, queue.completed_at) {
                    (Some(finished), Some(dequeued)) => {
                        finished.secs > dequeued.secs
                            || finished.secs == dequeued.secs && finished.nanos >= dequeued.nanos
                    }
                    _ => false,
                }
        }
    }
}

/// A timeout or a runner that never started is an infrastructure failure, not
/// a test failure; otherwise the report's own reason decides.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    requires !infra_failure(conclusion) ==> report.requires(()),
    ensures if infra_failure(conclusion) { result } else { report.ensures((), result) },
))]
pub fn ci_is_infrastructure(conclusion: Option<Conclusion>, report: impl FnOnce() -> bool) -> bool {
    matches!(
        conclusion,
        Some(Conclusion::TimedOut | Conclusion::StartupFailure)
    ) || report()
}

/// The selected queue check reports a successful merge through the queue.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    requires check.status == Status::Completed && check.conclusion == Some(Conclusion::Success) ==> state.requires(()),
    ensures result ==> check.status == Status::Completed && check.conclusion == Some(Conclusion::Success),
        check.status == Status::Completed && check.conclusion == Some(Conclusion::Success) ==>
            exists|r: QueueState| state.ensures((), r) && result == (r == QueueState::Merged),
))]
pub fn merged_via_queue(check: &CheckView, state: impl FnOnce() -> QueueState) -> bool {
    check.status == Status::Completed
        && check.conclusion == Some(Conclusion::Success)
        && state() == QueueState::Merged
}

#[cfg(verus_keep_ghost)]
verus! {
/// #743/K-observe-select: eligible current-head checks from the real app.
pub open spec fn qualifies(c: CheckView, name: Name) -> bool {
    c.head == Head::Current && c.name == name && c.app == App::Mergify
}
/// #743/K-observe-select: maximum ID with the last index winning equal IDs.
pub open spec fn selected(c: Seq<CheckView>, name: Name, n: int, result: Option<usize>) -> bool {
    0 <= n <= c.len() && match result {
        None => forall|j: int| 0 <= j < n ==> !qualifies(c[j], name),
        Some(i) => 0 <= i < n && qualifies(c[i as int], name)
            && forall|j: int| 0 <= j < n && qualifies(c[j], name) ==>
                c[j].id <= c[i as int].id && (c[j].id == c[i as int].id ==> j <= i),
    }
}
/// #743/K-observe-state: infrastructure overrides the report.
pub open spec fn infra_failure(c: Option<Conclusion>) -> bool {
    matches!(c, Some(Conclusion::TimedOut | Conclusion::StartupFailure))
}
/// #743/K-observe-merged/conflict: exact typed report classification.
pub open spec fn queue_result(p: PullView, r: Option<QueueState>) -> QueueState {
    let state = match r { Some(s) => s, None => QueueState::Unknown };
    if p.merged { QueueState::Merged }
    else if state == QueueState::Merged { QueueState::Unknown }
    else if state == QueueState::Dequeued && (p.mergeable == Some(false) || p.mergeable_state == Mergeable::Dirty) { QueueState::Conflict }
    else { state }
}
/// #743/K-observe-rule: independent existential over the newest qualifying rule.
pub open spec fn rule_result(c: Seq<CheckView>, queue: Option<usize>) -> bool {
    match queue {
        Some(q) => q < c.len() && exists|r: usize| selected(c, Name::QueueRule, c.len() as int, Some(r))
            && c[r as int].status == Status::Completed && c[r as int].conclusion.is_some()
            && match (c[r as int].completed_at, c[q as int].completed_at) {
                (Some(a), Some(b)) => a.secs > b.secs || a.secs == b.secs && a.nanos >= b.nanos,
                _ => false,
            },
        None => false,
    }
}
}
