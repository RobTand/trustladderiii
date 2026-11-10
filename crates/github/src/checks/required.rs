//! Required-check evaluation and workflow rerun decisions (ctrl-core C1).
//! The shell retains sorted keys, upstream payloads and every I/O boundary.
use super::Status;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(not(verus_keep_ghost))]
macro_rules! required_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => { while $condition $body };
}
#[cfg(verus_keep_ghost)]
macro_rules! required_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! { while $condition $($contract)* $body }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct Candidate {
    pub id: u64,
    pub eligible: bool,
}

/// Q52, #447, #2705; cdacce8f docs/state-table.md:542-545,578.
/// Unbounded selection within one shell-sorted key. Checks/statuses keep the
/// first equal ID; workflow runs use max_by_key's last-equal rule.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    match out {
        None => forall|j: int| 0 <= j < items.len() ==> !items[j].eligible,
        Some(i) => i < items.len() && items[i as int].eligible
            && forall|j: int| 0 <= j < items.len() && items[j].eligible ==>
                items[j].id <= items[i as int].id
                && (items[j].id == items[i as int].id ==>
                    if last_tie { j <= i } else { i <= j }),
    },
))]
pub fn latest(items: &[Candidate], last_tie: bool) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut i = 0;
    required_while! { (i < items.len()) [
        invariant i <= items.len(),
            match best {
                None => forall|j: int| 0 <= j < i ==> !items[j].eligible,
                Some(b) => b < i && items[b as int].eligible
                    && forall|j: int| 0 <= j < i && items[j].eligible ==>
                        items[j].id <= items[b as int].id
                        && (items[j].id == items[b as int].id ==>
                            if last_tie { j <= b } else { b <= j }),
            },
        decreases items.len() - i,
    ] {
        if items[i].eligible {
            let replace = match best {
                None => true,
                Some(b) => items[i].id > items[b].id
                    || last_tie && items[i].id == items[b].id,
            };
            if replace {
                best = Some(i);
            }
        }
        i += 1;
    }}
    best
}

/// Q52, #447, #2705; cdacce8f docs/state-table.md:543-545,555,578.
/// Every rule is inspected; an app-bound rule cannot consume a classic status.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (exists|i: int| 0 <= i < apps.len() && apps[i].is_none()),
))]
pub fn needs_statuses(apps: &[Option<u64>]) -> bool {
    let mut i = 0;
    required_while! { (i < apps.len()) [
        invariant i <= apps.len(), forall|j: int| 0 <= j < i ==> apps[j].is_some(),
        decreases apps.len() - i,
    ] {
        if apps[i].is_none() { return true; }
        i += 1;
    }}
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub enum ReadResult {
    Success,
    Http(u16),
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub enum StatusRead {
    Available,
    Unavailable,
    Propagate,
}

/// Q52, #447, #2705; cdacce8f docs/state-table.md:543-545,578.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (same_context && (expected.is_none() || expected == app)),
))]
pub fn rule_matches(same_context: bool, expected: Option<u64>, app: Option<u64>) -> bool {
    same_context
        && match expected {
            None => true,
            Some(id) => app == Some(id),
        }
}

/// Q52, #447, #2705; cdacce8f docs/state-table.md:541-542,578.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (completed && actions && failure),
))]
pub fn needs_annotations(completed: bool, actions: bool, failure: bool) -> bool {
    completed && actions && failure
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub enum Annotation {
    Ignore,
    RunnerFailure,
    OtherFailure,
}

/// Q52, #447, #2705; cdacce8f docs/state-table.md:541-542,578.
/// At least one failure, and every failure names a runner problem. No bound.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == ((exists|i: int| 0 <= i < annotations.len() && annotations[i] == Annotation::RunnerFailure)
        && (forall|i: int| 0 <= i < annotations.len() ==> annotations[i] != Annotation::OtherFailure)),
))]
pub fn runner_failure(annotations: &[Annotation]) -> bool {
    let mut any = false;
    let mut i = 0;
    required_while! { (i < annotations.len()) [
        invariant i <= annotations.len(),
            any == (exists|j: int| 0 <= j < i && annotations[j] == Annotation::RunnerFailure),
            forall|j: int| 0 <= j < i ==> annotations[j] != Annotation::OtherFailure,
        decreases annotations.len() - i,
    ] {
        match annotations[i] {
            Annotation::Ignore => {},
            Annotation::RunnerFailure => any = true,
            Annotation::OtherFailure => return false,
        }
        i += 1;
    }}
    any
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub enum Contribution {
    Passed,
    Pending,
    Infrastructure,
    Tests,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub enum Failure {
    None,
    Infrastructure,
    Tests,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct Summary {
    pub ready: bool,
    pub failure: Failure,
}

/// Q52, #447, #2705; cdacce8f docs/state-table.md:541-545,555,578.
/// Inductive aggregation step over any number of reports. Missing knowable
/// contexts contribute Pending; unknown absence contributes nothing, exactly
/// as frozen facts::required_missing specifies. The shell keeps failure payloads.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.ready == (before.ready && item == Contribution::Passed),
    out.failure == if before.failure == Failure::Tests || item == Contribution::Tests {
        Failure::Tests
    } else if before.failure == Failure::Infrastructure || item == Contribution::Infrastructure {
        Failure::Infrastructure
    } else { Failure::None },
))]
pub fn record(before: Summary, item: Contribution) -> Summary {
    let failure = if item == Contribution::Tests || before.failure == Failure::Tests {
        Failure::Tests
    } else if item == Contribution::Infrastructure || before.failure == Failure::Infrastructure {
        Failure::Infrastructure
    } else {
        Failure::None
    };
    Summary {
        ready: before.ready && item == Contribution::Passed,
        failure,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub enum ClassicStatus {
    Success,
    Failure,
    Error,
    Other,
}

/// Q52, #447, #2705; cdacce8f docs/state-table.md:541-545,555,578.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match status {
        ClassicStatus::Success => Contribution::Passed,
        ClassicStatus::Failure | ClassicStatus::Error => Contribution::Tests,
        ClassicStatus::Other => Contribution::Pending,
    },
))]
pub fn classic_status(status: ClassicStatus) -> Contribution {
    match status {
        ClassicStatus::Success => Contribution::Passed,
        ClassicStatus::Failure | ClassicStatus::Error => Contribution::Tests,
        ClassicStatus::Other => Contribution::Pending,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub enum DispatchResult {
    Accepted,
    InProgress,
    Forbidden,
    RateLimited,
    Http(u16),
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub enum RerunResult {
    Accepted,
    InProgress,
    Refused,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub enum Loop {
    ReadAgain,
    Done,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn required_aggregation_witness() {
        let start = Summary {
            ready: true,
            failure: Failure::None,
        };
        let infra = record(start, Contribution::Infrastructure);
        assert!(!infra.ready);
        assert_eq!(infra.failure, Failure::Infrastructure);
        assert_eq!(record(infra, Contribution::Tests).failure, Failure::Tests);
        assert_eq!(
            record(
                record(start, Contribution::Tests),
                Contribution::Infrastructure
            )
            .failure,
            Failure::Tests
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub enum RequiredState {
    Pending,
    Passed,
    Infrastructure,
    Tests,
    Unknown,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == match (check.status, check.conclusion) {
        (Status::Completed, Some(super::Conclusion::Success | super::Conclusion::Neutral | super::Conclusion::Skipped)) => RequiredState::Passed,
        (Status::Completed, Some(super::Conclusion::ActionRequired | super::Conclusion::Stale)) => RequiredState::Pending,
        (Status::Completed, Some(conclusion)) => if check.app == super::App::Actions
            && (matches!(conclusion, super::Conclusion::Cancelled | super::Conclusion::TimedOut | super::Conclusion::StartupFailure)
                || conclusion == super::Conclusion::Failure && runner_failure) {
            RequiredState::Infrastructure
        } else { RequiredState::Tests },
        (Status::Completed, None) => RequiredState::Unknown,
        _ => RequiredState::Pending,
    },
))]
pub fn required_check(check: super::CheckView, runner_failure: bool) -> RequiredState {
    use super::{App, Conclusion};
    if check.status != Status::Completed
        || matches!(
            check.conclusion,
            Some(Conclusion::ActionRequired | Conclusion::Stale)
        )
    {
        RequiredState::Pending
    } else if matches!(
        check.conclusion,
        Some(Conclusion::Success | Conclusion::Neutral | Conclusion::Skipped)
    ) {
        RequiredState::Passed
    } else if check.app == App::Actions
        && (matches!(
            check.conclusion,
            Some(Conclusion::Cancelled | Conclusion::TimedOut | Conclusion::StartupFailure)
        ) || check.conclusion == Some(Conclusion::Failure) && runner_failure)
    {
        RequiredState::Infrastructure
    } else if check.conclusion.is_some() {
        RequiredState::Tests
    } else {
        RequiredState::Unknown
    }
}
