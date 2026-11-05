//! Which queue facts one read of a PR yields. The adapter performs each read
//! these functions ask for, in the same order, and builds the facts.

#[cfg(verus_keep_ghost)]
use super::check::{grouping, kept};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

use super::check::{CheckView, Conclusion, Head, Status, newest_first};
use super::observe::QueueState;

/// What a queue read leads to before any further read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Entry {
    /// Mergify's own merge-queue PRs are never TL3's.
    Ignore,
    /// Someone else pushed a head TL3 did not expect.
    PushByOther,
    /// Read on. `head_checks` asks for the required checks on the head.
    Read { head_checks: bool },
}

pub fn entry(merge_queue_pr: bool, state: QueueState, expected_head: bool, open: bool) -> Entry {
    if merge_queue_pr {
        Entry::Ignore
    } else if state != QueueState::Merged && !expected_head {
        Entry::PushByOther
    } else {
        Entry::Read {
            head_checks: open
                && matches!(
                    state,
                    QueueState::Waiting
                        | QueueState::Unknown
                        | QueueState::FailedCi
                        | QueueState::Dequeued
                ),
        }
    }
}

/// Q52: a required context is missing when nothing reported it, neither a
/// check run from the expected app nor a status. Without an expected app only
/// a status can stand in, so an unreadable status list proves nothing.
pub fn required_missing(reported: bool, expects_app: bool, statuses_readable: bool) -> bool {
    !reported && (expects_app || statuses_readable)
}

/// Q52: every check suite on the head has completed, so no app is still
/// working on it and a missing required context will not report. No suites
/// at all proves nothing.
pub fn suites_completed(suites: &[Status]) -> bool {
    let mut i = 0;
    while i < suites.len() {
        if suites[i] != Status::Completed {
            return false;
        }
        i += 1;
    }
    !suites.is_empty()
}

/// #447: every check on the head, other than Mergify's own, has completed.
/// Until then a check suite read can't show a blocked queue.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == (forall|i: int| 0 <= i < checks@.len() ==> settled(checks@[i])),
))]
pub fn checks_settled(checks: &[CheckView]) -> bool {
    let mut i = 0;
    decide_while! { (i < checks.len()) [
        invariant i <= checks@.len(),
            forall|j: int| 0 <= j < i ==> settled(checks@[j]),
        decreases checks@.len() - i,
    ] {
        let check = &checks[i];
        if check.head == Head::Current && !check.app.mergify_slug() && !check.completed() {
            return false;
        }
        i += 1;
    } }
    true
}

/// #447: Mergify waits on queue conditions while every check suite on the
/// head has completed, so nothing still running can meet them. `unmet` says
/// the selected queue check names unmet conditions.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == (state == QueueState::Waiting && suites_completed && unmet),
))]
pub fn queue_blocked(state: QueueState, suites_completed: bool, unmet: bool) -> bool {
    state == QueueState::Waiting && suites_completed && unmet
}

/// #447: the newest check with each name on the head that failed, other than
/// Mergify's own and the required contexts, which have their own facts.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == (kept(checks@, i as int, false) && failure(checks@[i as int])),
))]
pub fn blocking_failure(checks: &[CheckView], i: usize) -> bool {
    let member = decide_predicate!(|check| check.head == Head::Current);
    let same = decide_predicate!(|a, b| a.name_key == b.name_key);
    let newest = newest_first(checks, i, member, same);
    decide_proof! { assert(grouping(checks@, member, same, false)); assert(newest == kept(checks@, i as int, false)); }
    newest && {
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

/// The merge commit's checks passed: the newest check per app and name on it
/// exists and every one completed with a passing conclusion. `Head::Current`
/// marks checks on the merge commit.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == ((exists|i: int| kept(checks@, i, true))
        && forall|i: int| kept(checks@, i, true) ==> passing(checks@[i])),
))]
pub fn checks_passed(checks: &[CheckView]) -> bool {
    let mut any = false;
    let mut i = 0;
    decide_while! { (i < checks.len()) [
        invariant i <= checks@.len(),
            any == (exists|j: int| 0 <= j < i && kept(checks@, j, true)),
            forall|j: int| 0 <= j < i && kept(checks@, j, true) ==> passing(checks@[j]),
        decreases checks@.len() - i,
    ] {
        let member = decide_predicate!(|check| check.head == Head::Current);
        let same = decide_predicate!(|a, b| a.app_key == b.app_key && a.name_key == b.name_key);
        let newest = newest_first(checks, i, member, same);
        decide_proof! { assert(grouping(checks@, member, same, true)); assert(newest == kept(checks@, i as int, true)); }
        if newest {
            any = true;
            let check = &checks[i];
            if !check.completed() || !match check.conclusion { Some(conclusion) => conclusion.passing(), None => false } {
                decide_proof! {
                    assert(kept(checks@, i as int, true));
                    assert(!passing(checks@[i as int]));
                    assert(!(forall|j: int| kept(checks@, j, true) ==> passing(checks@[j])));
                }
                return false;
            }
        }
        i += 1;
    } }
    any
}

/// The selected queue check, as the merge evidence reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct QueueCheck {
    pub completed: bool,
    pub concluded: bool,
    /// Its title is "Merged via merge queue".
    pub merged_title: bool,
}

/// Where a merge's `checks_passed` comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergedPlan {
    /// Mergify publishes its final queue result after GitHub records the
    /// merge, so wait for it. No fact yet.
    Wait,
    /// The queue check itself says whether the merge passed.
    ViaQueue,
    /// Read the checks on the merge commit.
    MergeCommit,
    /// Nothing shows the checks passed.
    NotPassed,
}

pub fn merged_plan(
    expected_head: bool,
    check: Option<QueueCheck>,
    merge_commit: bool,
) -> MergedPlan {
    if expected_head && check.is_some_and(|check| !check.completed || !check.concluded) {
        MergedPlan::Wait
    } else if check.is_some_and(|check| check.merged_title) {
        MergedPlan::ViaQueue
    } else if merge_commit {
        MergedPlan::MergeCommit
    } else {
        MergedPlan::NotPassed
    }
}

/// The fact a queue state yields once no head-check failure decided first.
/// Every outcome but a merge carries the selected queue check `C`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome<C> {
    Merged,
    CiFailed { check: C, infra: bool },
    DequeuedByCommand(C),
    Conflict(C),
}

/// `head_infra` is the head checks' own verdict, when they failed. `ci_infra`
/// reads the queue check's report and is called only when that decides.
pub fn queue_outcome<C: Copy>(
    state: QueueState,
    check: Option<C>,
    head_infra: Option<bool>,
    ci_infra: impl FnOnce(C) -> bool,
) -> Option<Outcome<C>> {
    match (state, check) {
        (QueueState::Merged, _) => Some(Outcome::Merged),
        (QueueState::FailedCi, Some(check)) => Some(Outcome::CiFailed {
            check,
            infra: head_infra.unwrap_or_else(|| ci_infra(check)),
        }),
        // Mergify dequeued it with no reason TL3 knows: treat the CI as broken.
        (QueueState::Dequeued, Some(check)) => Some(Outcome::CiFailed {
            check,
            infra: head_infra.unwrap_or(true),
        }),
        (QueueState::DequeuedByPerson, Some(check)) => Some(Outcome::DequeuedByCommand(check)),
        (QueueState::Conflict, Some(check)) => Some(Outcome::Conflict(check)),
        _ => None,
    }
}

/// Who acted in an issue event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Actor {
    Person,
    Bot,
    /// The event names no actor.
    Missing,
}

impl Actor {
    /// GitHub names every app's account `<slug>[bot]`.
    pub fn parse(login: Option<&str>) -> Self {
        match login {
            None => Self::Missing,
            Some(login) if login.ends_with("[bot]") => Self::Bot,
            Some(_) => Self::Person,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum EventKind {
    /// A label was removed; `queue_label` says it was TL3's queue label.
    Unlabeled {
        queue_label: bool,
    },
    Closed,
    Merged,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct IssueEvent {
    pub id: u64,
    pub actor: Actor,
    pub kind: EventKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonFact {
    /// A person removed the queue label.
    Dequeued { event: u64 },
    /// A person closed the PR.
    Closed { event: u64 },
}

/// The fact one event shows, if a person acted.
pub fn person_fact(event: IssueEvent) -> Option<PersonFact> {
    if event.actor != Actor::Person {
        return None;
    }
    match event.kind {
        EventKind::Unlabeled { queue_label: true } => {
            Some(PersonFact::Dequeued { event: event.id })
        }
        EventKind::Closed => Some(PersonFact::Closed { event: event.id }),
        _ => None,
    }
}

/// A person's dequeue or close, in event order. A merged PR has none: the
/// merge decides.
pub fn person_facts(events: &[IssueEvent], merged: bool) -> impl Iterator<Item = PersonFact> + '_ {
    let merged = merged || events.iter().any(|event| event.kind == EventKind::Merged);
    events
        .iter()
        .filter(move |_| !merged)
        .filter_map(|event| person_fact(*event))
}

#[cfg(verus_keep_ghost)]
verus! {
/// #447/K-queue-blocked: only current non-Mergify checks must complete.
pub open spec fn settled(c: CheckView) -> bool {
    c.head != Head::Current || matches!(c.app, super::check::App::Mergify | super::check::App::MergifySlug)
        || c.status == Status::Completed
}
/// #743/K-checks-passed: exactly success, skipped and neutral pass.
pub open spec fn passing(c: CheckView) -> bool {
    c.status == Status::Completed && matches!(c.conclusion, Some(Conclusion::Success | Conclusion::Skipped | Conclusion::Neutral))
}
/// #447/K-queue-blocked: required contexts and the Mergify slug have separate handling.
pub open spec fn failure(c: CheckView) -> bool {
    c.status == Status::Completed
        && matches!(c.conclusion, Some(Conclusion::Failure | Conclusion::Cancelled | Conclusion::TimedOut | Conclusion::ActionRequired | Conclusion::StartupFailure))
        && !matches!(c.app, super::check::App::Mergify | super::check::App::MergifySlug) && !c.required
}
}
