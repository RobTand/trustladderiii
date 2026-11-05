//! #743 review r1: compare every rewritten decision with its verbatim base body.
//! References are integration-test modules and never compiled into the library.

#[path = "observe_differential/reference.rs"]
mod reference;

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

use proptest::prelude::*;
use tl3_mergify::decide::check::{self, App, CheckView, Conclusion, Head, Name, Status, Time};
use tl3_mergify::decide::facts;
use tl3_mergify::decide::observe::{self, Mergeable, PullView, QueueState};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Call {
    Report,
    Member(CheckView),
    Same(CheckView, CheckView),
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

fn callback(calls: &RefCell<Vec<Call>>, call: Call, panic_at: Option<usize>) -> usize {
    let index = calls.borrow().len();
    calls.borrow_mut().push(call);
    assert_ne!(Some(index), panic_at, "injected callback panic");
    index
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

fn conclusion() -> impl Strategy<Value = Option<Conclusion>> {
    prop::option::of(prop::sample::select(vec![
        Conclusion::Success,
        Conclusion::Failure,
        Conclusion::Neutral,
        Conclusion::Cancelled,
        Conclusion::Skipped,
        Conclusion::TimedOut,
        Conclusion::ActionRequired,
        Conclusion::StartupFailure,
        Conclusion::Stale,
        Conclusion::Other,
    ]))
}

fn name() -> impl Strategy<Value = Name> {
    prop::sample::select(vec![Name::QueueCheck, Name::QueueRule, Name::Other])
}

fn queue_state() -> impl Strategy<Value = QueueState> {
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

fn time() -> impl Strategy<Value = Time> {
    (
        prop_oneof![
            any::<i64>(),
            prop::sample::select(vec![i64::MIN, -1, 0, 1, i64::MAX])
        ],
        prop_oneof![
            any::<u32>(),
            prop::sample::select(vec![0, 1, 999_999_999, 1_000_000_000, u32::MAX])
        ],
    )
        .prop_map(|(secs, nanos)| Time { secs, nanos })
}

fn check() -> impl Strategy<Value = CheckView> {
    (
        prop_oneof![any::<u64>(), 0u64..4, Just(u64::MAX)],
        any::<bool>(),
        prop::sample::select(vec![
            App::Mergify,
            App::MergifySlug,
            App::Actions,
            App::Other,
            App::Missing,
        ]),
        prop_oneof![any::<u32>(), 0u32..3],
        name(),
        prop_oneof![any::<u32>(), 0u32..3],
        any::<bool>(),
        status(),
        conclusion(),
        prop::option::of(time()),
    )
        .prop_map(
            |(
                id,
                current,
                app,
                app_key,
                name,
                name_key,
                required,
                status,
                conclusion,
                completed_at,
            )| CheckView {
                id,
                head: if current { Head::Current } else { Head::Other },
                app,
                app_key,
                name,
                name_key,
                required,
                status,
                conclusion,
                completed_at,
            },
        )
}

fn checks() -> impl Strategy<Value = Vec<CheckView>> {
    prop::collection::vec(check(), 0..17)
}

fn index() -> impl Strategy<Value = usize> {
    prop_oneof![0usize..20, any::<usize>(), Just(usize::MAX)]
}

fn pull() -> impl Strategy<Value = PullView> {
    (
        any::<bool>(),
        prop::option::of(any::<bool>()),
        prop::sample::select(vec![
            Mergeable::Missing,
            Mergeable::Unknown,
            Mergeable::Dirty,
            Mergeable::Other,
        ]),
    )
        .prop_map(|(merged, mergeable, mergeable_state)| PullView {
            merged,
            mergeable,
            mergeable_state,
        })
}

// Run both ordinary grouping callbacks and stateful, possibly panicking ones.
fn newest(
    old: bool,
    checks: &[CheckView],
    i: usize,
    by_app: bool,
    script: Option<&[bool]>,
    panic_at: Option<usize>,
) -> Observation<bool> {
    record(|calls| {
        let member = |check: &CheckView| {
            let n = callback(calls, Call::Member(*check), panic_at);
            script.map_or(check.head == Head::Current, |s| s[n % s.len()])
        };
        let same = |a: &CheckView, b: &CheckView| {
            let n = callback(calls, Call::Same(*a, *b), panic_at);
            script.map_or(
                a.name_key == b.name_key && (!by_app || a.app_key == b.app_key),
                |s| s[n % s.len()],
            )
        };
        if old {
            reference::newest_first(checks, i, member, same)
        } else {
            check::newest_first(checks, i, member, same)
        }
    })
}

fn rule_pair(queue: Option<Time>, finished: Option<Time>) -> [CheckView; 2] {
    let base = CheckView {
        id: 0,
        head: Head::Current,
        app: App::Mergify,
        app_key: 0,
        name: Name::QueueCheck,
        name_key: 0,
        required: false,
        status: Status::Completed,
        conclusion: Some(Conclusion::Success),
        completed_at: queue,
    };
    [
        base,
        CheckView {
            name: Name::QueueRule,
            name_key: 1,
            completed_at: finished,
            ..base
        },
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn select_matches_base(checks in checks(), name in name()) {
        prop_assert_eq!(record(|_| reference::select(&checks, name)), record(|_| observe::select(&checks, name)));
    }

    #[test]
    fn select_equal_ids_keep_the_last(id in any::<u64>()) {
        let mut checks = rule_pair(None, None);
        checks[0].id = id;
        checks[1] = checks[0];
        prop_assert_eq!(record(|_| reference::select(&checks, Name::QueueCheck)), record(|_| observe::select(&checks, Name::QueueCheck)));
    }

    #[test]
    fn queue_state_matches_base(pull in pull(), reported in prop::option::of(queue_state()), panic in any::<bool>()) {
        let run = |old| record(|calls| {
            let report = || {
                callback(calls, Call::Report, if panic { Some(0) } else { None });
                reported
            };
            if old { reference::queue_state(pull, report) } else { observe::queue_state(pull, report) }
        });
        prop_assert_eq!(run(true), run(false));
    }

    #[test]
    fn rule_concluded_matches_base(checks in checks(), queue in prop::option::of(index())) {
        prop_assert_eq!(record(|_| reference::rule_concluded(&checks, queue)), record(|_| observe::rule_concluded(&checks, queue)));
    }

    #[test]
    fn rule_concluded_full_timestamps(queue in prop::option::of(time()), finished in prop::option::of(time())) {
        let checks = rule_pair(queue, finished);
        prop_assert_eq!(record(|_| reference::rule_concluded(&checks, Some(0))), record(|_| observe::rule_concluded(&checks, Some(0))));
    }

    #[test]
    fn newest_first_matches_base(
        checks in checks(), i in index(), by_app in any::<bool>(),
        script in prop::collection::vec(any::<bool>(), 1..9),
        panic_at in prop::option::of(0usize..35),
    ) {
        for script in [None, Some(script.as_slice())] {
            prop_assert_eq!(newest(true, &checks, i, by_app, script, panic_at), newest(false, &checks, i, by_app, script, panic_at));
        }
    }

    #[test]
    fn checks_settled_matches_base(checks in checks()) {
        prop_assert_eq!(record(|_| reference::checks_settled(&checks)), record(|_| facts::checks_settled(&checks)));
    }

    #[test]
    fn blocking_failure_matches_base(checks in checks(), i in index()) {
        prop_assert_eq!(record(|_| reference::blocking_failure(&checks, i)), record(|_| facts::blocking_failure(&checks, i)));
    }

    #[test]
    fn checks_passed_matches_base(checks in checks()) {
        prop_assert_eq!(record(|_| reference::checks_passed(&checks)), record(|_| facts::checks_passed(&checks)));
    }
}

#[test]
fn rule_concluded_timestamp_edges() {
    let times = [
        None,
        Some(Time {
            secs: i64::MIN,
            nanos: 0,
        }),
        Some(Time {
            secs: i64::MIN,
            nanos: u32::MAX,
        }),
        Some(Time {
            secs: -1,
            nanos: u32::MAX,
        }),
        Some(Time { secs: 0, nanos: 0 }),
        Some(Time { secs: 0, nanos: 1 }),
        Some(Time {
            secs: 0,
            nanos: u32::MAX,
        }),
        Some(Time { secs: 1, nanos: 0 }),
        Some(Time {
            secs: i64::MAX,
            nanos: 0,
        }),
        Some(Time {
            secs: i64::MAX,
            nanos: u32::MAX,
        }),
    ];
    for queue in times {
        for finished in times {
            let checks = rule_pair(queue, finished);
            for i in [None, Some(0), Some(1), Some(2), Some(usize::MAX)] {
                assert_eq!(
                    record(|_| reference::rule_concluded(&checks, i)),
                    record(|_| observe::rule_concluded(&checks, i)),
                    "queue={queue:?}, finished={finished:?}, index={i:?}"
                );
            }
        }
    }
}

#[test]
fn newest_first_trace_and_panic_edges() {
    let mut checks = rule_pair(None, None);
    checks[1] = checks[0];
    for i in [0, 1, 2, usize::MAX] {
        let expected = newest(true, &checks, i, true, None, None);
        assert_eq!(expected, newest(false, &checks, i, true, None, None));
        assert!(expected.result.is_ok());
        for position in 0..expected.calls.len() {
            let expected = newest(true, &checks, i, true, None, Some(position));
            assert!(expected.result.is_err());
            assert_eq!(
                expected,
                newest(false, &checks, i, true, None, Some(position))
            );
        }
        if i >= checks.len() {
            assert!(expected.calls.is_empty());
        }
    }
}
