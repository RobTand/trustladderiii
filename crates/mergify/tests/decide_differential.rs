//! #743 M1b: preserve pre-port results, panics, callbacks and lazy iteration.
#[path = "decide_differential/reference.rs"]
mod reference;

use proptest::prelude::*;
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use tl3_mergify::decide::check::Status;
use tl3_mergify::decide::facts::{self, Actor, EventKind, IssueEvent, PersonFact, QueueCheck};
use tl3_mergify::decide::observe::QueueState;

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Report(u64),
    Yield(Option<PersonFact>),
    Hint((usize, Option<usize>)),
}

#[derive(Debug, PartialEq, Eq)]
struct Observation<T> {
    result: Result<T, String>,
    calls: Vec<Call>,
}

fn record<T>(run: impl FnOnce(&RefCell<Vec<Call>>) -> T) -> Observation<T> {
    let calls = RefCell::new(Vec::new());
    let result = catch_unwind(AssertUnwindSafe(|| run(&calls))).map_err(|panic| {
        if let Some(message) = panic.downcast_ref::<&str>() {
            (*message).to_owned()
        } else if let Some(message) = panic.downcast_ref::<String>() {
            message.clone()
        } else {
            "non-string panic".to_owned()
        }
    });
    Observation {
        result,
        calls: calls.into_inner(),
    }
}

fn status() -> impl Strategy<Value = Status> {
    prop::sample::select(vec![
        Status::Queued,
        Status::InProgress,
        Status::Completed,
        Status::Waiting,
        Status::Requested,
        Status::Pending,
        Status::Other,
    ])
}
fn state() -> impl Strategy<Value = QueueState> {
    prop::sample::select(vec![
        QueueState::Waiting,
        QueueState::Queued,
        QueueState::ChecksRunning,
        QueueState::Merged,
        QueueState::Dequeued,
        QueueState::DequeuedByPerson,
        QueueState::FailedCi,
        QueueState::Conflict,
        QueueState::Unknown,
    ])
}
fn queue_check() -> impl Strategy<Value = QueueCheck> {
    (any::<bool>(), any::<bool>(), any::<bool>()).prop_map(
        |(completed, concluded, merged_title)| QueueCheck {
            completed,
            concluded,
            merged_title,
        },
    )
}
fn event() -> impl Strategy<Value = IssueEvent> {
    (
        any::<u64>(),
        prop::sample::select(vec![Actor::Person, Actor::Bot, Actor::Missing]),
        prop::sample::select(vec![
            EventKind::Unlabeled { queue_label: false },
            EventKind::Unlabeled { queue_label: true },
            EventKind::Closed,
            EventKind::Merged,
            EventKind::Other,
        ]),
    )
        .prop_map(|(id, actor, kind)| IssueEvent { id, actor, kind })
}
fn person_run(
    old: bool,
    events: &[IssueEvent],
    merged: bool,
    skips: &[usize],
    panic_at: Option<usize>,
) -> Observation<Vec<PersonFact>> {
    record(|calls| {
        let mut iter: Box<dyn Iterator<Item = PersonFact>> = if old {
            Box::new(reference::person_facts(events, merged))
        } else {
            Box::new(facts::person_facts(events, merged))
        };
        let mut out = Vec::new();
        calls.borrow_mut().push(Call::Hint(iter.size_hint()));
        for (n, skip) in skips.iter().enumerate() {
            let fact = iter.nth(*skip);
            calls.borrow_mut().push(Call::Yield(fact));
            calls.borrow_mut().push(Call::Hint(iter.size_hint()));
            assert_ne!(Some(n), panic_at, "consumer panic");
            if let Some(fact) = fact {
                out.push(fact);
            }
        }
        for fact in iter {
            calls.borrow_mut().push(Call::Yield(Some(fact)));
            out.push(fact);
        }
        out
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]
    #[test]
    fn suites_completed_matches_base(suites in prop::collection::vec(status(), 0..65)) {
        prop_assert_eq!(record(|_| reference::suites_completed(&suites)), record(|_| facts::suites_completed(&suites)));
    }
    #[test]
    fn merged_plan_matches_base(expected in any::<bool>(), check in prop::option::of(queue_check()), commit in any::<bool>()) {
        prop_assert_eq!(record(|_| reference::merged_plan(expected, check, commit)), record(|_| facts::merged_plan(expected, check, commit)));
    }
    #[test]
    fn queue_outcome_matches_base(state in state(), check in prop::option::of(any::<u64>()), head in prop::option::of(any::<bool>()), report in any::<bool>(), panic in any::<bool>()) {
        let run = |old| record(|calls| {
            let callback = |check| {
                calls.borrow_mut().push(Call::Report(check));
                assert!(!panic, "report panic");
                report
            };
            if old { reference::queue_outcome(state, check, head, callback) }
            else { facts::queue_outcome(state, check, head, callback) }
        });
        prop_assert_eq!(run(true), run(false));
    }
    #[test]
    fn person_facts_matches_base(events in prop::collection::vec(event(), 0..65), merged in any::<bool>(), skips in prop::collection::vec(0usize..5, 0..70), panic_at in prop::option::of(0usize..70)) {
        prop_assert_eq!(person_run(true, &events, merged, &skips, panic_at), person_run(false, &events, merged, &skips, panic_at));
    }
}

#[test]
fn person_order_exhaustion_merge_and_panics() {
    let mut events = [
        IssueEvent {
            id: u64::MAX,
            actor: Actor::Person,
            kind: EventKind::Unlabeled { queue_label: true },
        },
        IssueEvent {
            id: 0,
            actor: Actor::Bot,
            kind: EventKind::Closed,
        },
        IssueEvent {
            id: u64::MAX,
            actor: Actor::Person,
            kind: EventKind::Closed,
        },
    ];
    for merged in [false, true] {
        for skips in [&[0, 0, 0, 0][..], &[1, 0, 0][..], &[3, 0][..]] {
            for panic_at in [None, Some(0), Some(1), Some(2)] {
                assert_eq!(
                    person_run(true, &events, merged, skips, panic_at),
                    person_run(false, &events, merged, skips, panic_at)
                );
            }
        }
    }
    events[2].kind = EventKind::Merged;
    assert_eq!(
        person_run(true, &events, false, &[0, 0], None),
        person_run(false, &events, false, &[0, 0], None)
    );
}
