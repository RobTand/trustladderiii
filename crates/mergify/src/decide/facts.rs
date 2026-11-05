//! Which queue facts one read of a PR yields. The adapter performs each read
//! these functions ask for, in the same order, and builds the facts.

#[cfg(verus_keep_ghost)]
use super::check::{grouping, kept};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

use super::check::{CheckView, Conclusion, Head, Status, newest_first};
use super::observe::QueueState;

/// What a queue read leads to before any further read.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
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

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == (if merge_queue_pr { Entry::Ignore } else if state != QueueState::Merged && !expected_head { Entry::PushByOther } else { Entry::Read { head_checks: open && matches!(state, QueueState::Waiting | QueueState::Unknown | QueueState::FailedCi | QueueState::Dequeued) } }),
))]
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
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == (!reported && (expects_app || statuses_readable)),
))]
pub fn required_missing(reported: bool, expects_app: bool, statuses_readable: bool) -> bool {
    !reported && (expects_app || statuses_readable)
}

/// Q52: every check suite on the head has completed, so no app is still
/// working on it and a missing required context will not report. No suites
/// at all proves nothing.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == (suites@.len() > 0 && forall|i: int| 0 <= i < suites@.len() ==> suites@[i] == Status::Completed),
))]
pub fn suites_completed(suites: &[Status]) -> bool {
    let mut i = 0;
    decide_while! { (i < suites.len()) [
        invariant i <= suites@.len(), forall|j: int| 0 <= j < i ==> suites@[j] == Status::Completed,
        decreases suites@.len() - i,
    ] {
        if suites[i] != Status::Completed {
            return false;
        }
        i += 1;
    } }
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
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct QueueCheck {
    pub completed: bool,
    pub concluded: bool,
    /// Its title is "Merged via merge queue".
    pub merged_title: bool,
}

/// Where a merge's `checks_passed` comes from.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
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

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == merged_result(expected_head, check, merge_commit),
))]
pub fn merged_plan(
    expected_head: bool,
    check: Option<QueueCheck>,
    merge_commit: bool,
) -> MergedPlan {
    if expected_head
        && match check {
            Some(check) => !check.completed || !check.concluded,
            None => false,
        }
    {
        MergedPlan::Wait
    } else if match check {
        Some(check) => check.merged_title,
        None => false,
    } {
        MergedPlan::ViaQueue
    } else if merge_commit {
        MergedPlan::MergeCommit
    } else {
        MergedPlan::NotPassed
    }
}

/// The fact a queue state yields once no head-check failure decided first.
/// Every outcome but a merge carries the selected queue check `C`.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome<C> {
    Merged,
    CiFailed { check: C, infra: bool },
    DequeuedByCommand(C),
    Conflict(C),
}

/// `head_infra` is the head checks' own verdict, when they failed. `ci_infra`
/// reads the queue check's report and is called only when that decides.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    requires (state == QueueState::FailedCi && check.is_some() && head_infra.is_none()) ==> ci_infra.requires((check.unwrap(),)),
    ensures if state == QueueState::FailedCi && check.is_some() && head_infra.is_none() { exists|r: bool| ci_infra.ensures((check.unwrap(),), r) && result == outcome_result(state, check, head_infra, r) } else { result == outcome_result(state, check, head_infra, false) },
))]
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
            infra: match head_infra {
                Some(infra) => infra,
                None => {
                    let infra = ci_infra(check);
                    decide_proof! {
                        assert(ci_infra.ensures((check,), infra));
                        assert(exists|r: bool| ci_infra.ensures((check,), r)
                            && Some(Outcome::CiFailed { check, infra }) == outcome_result(state, Some(check), head_infra, r));
                    }
                    infra
                }
            },
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
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
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

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
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

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct IssueEvent {
    pub id: u64,
    pub actor: Actor,
    pub kind: EventKind,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonFact {
    /// A person removed the queue label.
    Dequeued { event: u64 },
    /// A person closed the PR.
    Closed { event: u64 },
}

/// The fact one event shows, if a person acted.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == person_result(event),
))]
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
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result.valid(), result.remaining() ==
        if merged || exists|i: int| 0 <= i < events@.len() && events@[i].kind == EventKind::Merged {
            Seq::<PersonFact>::empty()
        } else { person_sequence(events@, 0) },
))]
pub fn person_facts(events: &[IssueEvent], merged: bool) -> PersonFacts<'_> {
    decide_proof! { reveal(PersonFacts::valid); reveal(PersonFacts::remaining); }
    let mut merge_seen = merged;
    if !merged {
        let mut i = 0;
        decide_while! { (i < events.len() && !merge_seen) [
            invariant i <= events@.len(), merge_seen == (exists|j: int| 0 <= j < i && events@[j].kind == EventKind::Merged),
            decreases events@.len() - i,
        ] {
            if events[i].kind == EventKind::Merged {
                merge_seen = true;
            }
            i += 1;
        } }
    }
    PersonFacts {
        events,
        position: 0,
        suppressed: merge_seen,
    }
}

/// #743/K-person-facts: allocation-free lazy event cursor.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct PersonFacts<'a> {
    events: &'a [IssueEvent],
    position: usize,
    suppressed: bool,
}

impl PersonFacts<'_> {
    /// #743/K-person-facts: next fact preserves event order and merge suppression.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
        requires old(self).valid(),
        ensures final(self).valid(),
            match result {
                Some(fact) => old(self).remaining() == seq![fact] + final(self).remaining(),
                None => old(self).remaining().len() == 0 && final(self).remaining().len() == 0,
            },
    ))]
    pub fn next_fact(&mut self) -> Option<PersonFact> {
        decide_proof! { reveal(PersonFacts::valid); reveal(PersonFacts::remaining); }
        if self.suppressed {
            self.position = self.events.len();
            return None;
        }
        let start = self.position;
        let mut i = start;
        decide_while! { (i < self.events.len()) [
            invariant start <= i <= self.events@.len(), self.position == start,
                !self.suppressed,
                person_sequence(self.events@, i as int) == person_sequence(self.events@, start as int),
            decreases self.events@.len() - i,
        ] {
            let fact = person_fact(self.events[i]);
            decide_proof! { reveal(person_sequence); }
            i += 1;
            if fact.is_some() {
                self.position = i;
                return fact;
            }
        } }
        self.position = i;
        None
    }
}

// The ordinary Rust trait bridge delegates every decision to the verified cursor.
#[cfg(not(verus_keep_ghost))]
impl Iterator for PersonFacts<'_> {
    type Item = PersonFact;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_fact()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.events.len() - self.position))
    }
}

#[cfg(verus_keep_ghost)]
verus! {
impl PersonFacts<'_> {
    /// #743/K-person-facts: the cursor always names a slice suffix.
    pub closed spec fn valid(&self) -> bool { self.position <= self.events@.len() }
    /// #743/K-person-facts: exact remaining event-ordered output.
    pub closed spec fn remaining(&self) -> Seq<PersonFact> {
        if self.suppressed { seq![] } else { person_sequence(self.events@, self.position as int) }
    }
}
/// #743/K-person-facts: independent per-event decision oracle.
pub open spec fn person_result(event: IssueEvent) -> Option<PersonFact> {
    match (event.actor, event.kind) {
        (Actor::Person, EventKind::Unlabeled { queue_label: true }) => Some(PersonFact::Dequeued { event: event.id }),
        (Actor::Person, EventKind::Closed) => Some(PersonFact::Closed { event: event.id }),
        _ => None,
    }
}
/// #743/K-person-facts: output is an ordered subsequence, at most one per event.
pub open spec fn person_sequence(events: Seq<IssueEvent>, start: int) -> Seq<PersonFact>
    decreases events.len() - start,
{
    if start < 0 || start >= events.len() { seq![] }
    else { match person_result(events[start]) {
        Some(fact) => seq![fact] + person_sequence(events, start + 1),
        None => person_sequence(events, start + 1),
    } }
}
/// #743/K-facts-outcome: exact merge evidence priority, independent of library helpers.
pub open spec fn merged_result(expected: bool, check: Option<QueueCheck>, commit: bool) -> MergedPlan {
    if expected && match check { Some(c) => !c.completed || !c.concluded, None => false } { MergedPlan::Wait }
    else if match check { Some(c) => c.merged_title, None => false } { MergedPlan::ViaQueue }
    else if commit { MergedPlan::MergeCommit } else { MergedPlan::NotPassed }
}
/// #743/K-facts-outcome: queue payloads and infrastructure priority are preserved.
pub open spec fn outcome_result<C>(state: QueueState, check: Option<C>, head: Option<bool>, report: bool) -> Option<Outcome<C>> {
    match (state, check) {
        (QueueState::Merged, _) => Some(Outcome::Merged),
        (QueueState::FailedCi, Some(check)) => Some(Outcome::CiFailed { check, infra: match head { Some(v) => v, None => report } }),
        (QueueState::Dequeued, Some(check)) => Some(Outcome::CiFailed { check, infra: match head { Some(v) => v, None => true } }),
        (QueueState::DequeuedByPerson, Some(check)) => Some(Outcome::DequeuedByCommand(check)),
        (QueueState::Conflict, Some(check)) => Some(Outcome::Conflict(check)),
        _ => None,
    }
}
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
