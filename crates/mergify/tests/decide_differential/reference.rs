//! Verbatim pre-M1b bodies from df6cb536. Test-only references.
use tl3_mergify::decide::check::Status;
use tl3_mergify::decide::facts::{
    Actor, EventKind, IssueEvent, MergedPlan, Outcome, PersonFact, QueueCheck,
};
use tl3_mergify::decide::observe::QueueState;

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

pub fn person_facts(events: &[IssueEvent], merged: bool) -> impl Iterator<Item = PersonFact> + '_ {
    let merged = merged || events.iter().any(|event| event.kind == EventKind::Merged);
    events
        .iter()
        .filter(move |_| !merged)
        .filter_map(|event| person_fact(*event))
}
