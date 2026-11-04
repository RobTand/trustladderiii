//! Which queue check describes a PR, and what state the queue is in.

use super::check::{App, CheckView, Conclusion, Head, Name, Status};

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
pub fn select(checks: &[CheckView], name: Name) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut i = 0;
    while i < checks.len() {
        let check = &checks[i];
        if check.head == Head::Current
            && check.name == name
            && check.app == App::Mergify
            && best.is_none_or(|best| check.id >= checks[best].id)
        {
            best = Some(i);
        }
        i += 1;
    }
    best
}

/// A timeout or a runner that never started fails the queue's CI whatever the
/// report says, and `report` is not read. Otherwise the report decides.
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
pub fn queue_state(pull: PullView, reported: impl FnOnce() -> Option<QueueState>) -> QueueState {
    if pull.merged {
        return QueueState::Merged;
    }
    let reported = reported().unwrap_or(QueueState::Unknown);
    if reported == QueueState::Merged {
        QueueState::Unknown
    } else if reported == QueueState::Dequeued
        && (pull.mergeable == Some(false) || pull.mergeable_state == Mergeable::Dirty)
    {
        QueueState::Conflict
    } else {
        reported
    }
}

/// A dequeue can't be told from a conflict until GitHub computes mergeability.
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
pub fn rule_concluded(checks: &[CheckView], queue: Option<usize>) -> bool {
    let Some(queue) = queue.and_then(|queue| checks.get(queue)) else {
        return false;
    };
    select(checks, Name::QueueRule).is_some_and(|rule| {
        let rule = &checks[rule];
        rule.status == Status::Completed
            && rule.conclusion.is_some()
            && match (rule.completed_at, queue.completed_at) {
                (Some(finished), Some(dequeued)) => finished >= dequeued,
                _ => false,
            }
    })
}

/// A timeout or a runner that never started is an infrastructure failure, not
/// a test failure; otherwise the report's own reason decides.
pub fn ci_is_infrastructure(conclusion: Option<Conclusion>, report: impl FnOnce() -> bool) -> bool {
    matches!(
        conclusion,
        Some(Conclusion::TimedOut | Conclusion::StartupFailure)
    ) || report()
}

/// The selected queue check reports a successful merge through the queue.
pub fn merged_via_queue(check: &CheckView, state: impl FnOnce() -> QueueState) -> bool {
    check.status == Status::Completed
        && check.conclusion == Some(Conclusion::Success)
        && state() == QueueState::Merged
}
