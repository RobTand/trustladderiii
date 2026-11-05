//! Verbatim pre-Verus functions from 4a7fa87a835375d288309d844347c343af70d3b5.
//! This module belongs only to the differential integration test.

use tl3_mergify::decide::check::{App, CheckView, Conclusion, Head, Name, Status};
use tl3_mergify::decide::observe::{Mergeable, PullView, QueueState};

pub fn newest_first(
    checks: &[CheckView],
    i: usize,
    member: impl Fn(&CheckView) -> bool,
    same: impl Fn(&CheckView, &CheckView) -> bool,
) -> bool {
    let Some(check) = checks.get(i) else {
        return false;
    };
    if !member(check) {
        return false;
    }
    let mut j = 0;
    while j < checks.len() {
        let other = &checks[j];
        if j != i
            && member(other)
            && same(other, check)
            && (other.id > check.id || j < i && other.id == check.id)
        {
            return false;
        }
        j += 1;
    }
    true
}

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

pub fn checks_settled(checks: &[CheckView]) -> bool {
    let mut i = 0;
    while i < checks.len() {
        let check = &checks[i];
        if check.head == Head::Current && !check.app.mergify_slug() && !check.completed() {
            return false;
        }
        i += 1;
    }
    true
}

pub fn blocking_failure(checks: &[CheckView], i: usize) -> bool {
    newest_first(
        checks,
        i,
        |check| check.head == Head::Current,
        |a, b| a.name_key == b.name_key,
    ) && {
        let check = &checks[i];
        check.completed()
            && matches!(
                check.conclusion,
                Some(
                    Conclusion::Failure
                        | Conclusion::Cancelled
                        | Conclusion::TimedOut
                        | Conclusion::ActionRequired
                        | Conclusion::StartupFailure
                )
            )
            && !check.app.mergify_slug()
            && !check.required
    }
}

pub fn checks_passed(checks: &[CheckView]) -> bool {
    let mut any = false;
    let mut i = 0;
    while i < checks.len() {
        if newest_first(
            checks,
            i,
            |check| check.head == Head::Current,
            |a, b| a.app_key == b.app_key && a.name_key == b.name_key,
        ) {
            any = true;
            let check = &checks[i];
            if !check.completed() || !check.conclusion.is_some_and(Conclusion::passing) {
                return false;
            }
        }
        i += 1;
    }
    any
}
