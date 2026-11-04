//! Boundary parsers: GitHub's strings into the queue decisions' enums (#735).
//! Every named value round-trips; any other string is `Other`, never a
//! named value; parsing never panics. Strategies mix named values and near
//! misses with arbitrary strings, so a loosened match fails.

use chrono::{DateTime, Utc};
use proptest::prelude::*;
use proptest::test_runner::Config;
use tl3_github::{CheckApp, CheckRun};
use tl3_mergify::decide::facts::Actor;
use tl3_mergify::decide::observe::Mergeable;
use tl3_mergify::decide::{
    App, Conclusion, Head, MERGIFY_APP_ID, Name, QUEUE_CHECK, QUEUE_RULE_CHECK, Status, Time,
};

const STATUS_NAMES: [&str; 6] = [
    "queued",
    "in_progress",
    "completed",
    "waiting",
    "requested",
    "pending",
];
const CONCLUSION_NAMES: [&str; 9] = [
    "success",
    "failure",
    "neutral",
    "cancelled",
    "skipped",
    "timed_out",
    "action_required",
    "startup_failure",
    "stale",
];

#[test]
fn named_values_round_trip() {
    for (status, name) in Status::NAMED.into_iter().zip(STATUS_NAMES) {
        assert_eq!(status.name(), Some(name));
        assert_eq!(Status::parse(name), status);
    }
    for (conclusion, name) in Conclusion::NAMED.into_iter().zip(CONCLUSION_NAMES) {
        assert_eq!(conclusion.name(), Some(name));
        assert_eq!(Conclusion::parse(name), conclusion);
    }
    assert_eq!(Status::Other.name(), None);
    assert_eq!(Conclusion::Other.name(), None);
    assert_eq!(Name::parse(QUEUE_CHECK), Name::QueueCheck);
    assert_eq!(Name::parse(QUEUE_RULE_CHECK), Name::QueueRule);
    assert_eq!(App::parse(Some((MERGIFY_APP_ID, "mergify"))), App::Mergify);
    assert_eq!(App::parse(Some((1, "mergify"))), App::MergifySlug);
    assert_eq!(App::parse(Some((15368, "github-actions"))), App::Actions);
    assert_eq!(App::parse(None), App::Missing);
    assert_eq!(Mergeable::parse(None), Mergeable::Missing);
    assert_eq!(Mergeable::parse(Some("unknown")), Mergeable::Unknown);
    assert_eq!(Mergeable::parse(Some("dirty")), Mergeable::Dirty);
    assert_eq!(Actor::parse(Some("mergify[bot]")), Actor::Bot);
    assert_eq!(Actor::parse(Some("RobTand")), Actor::Person);
    assert_eq!(Actor::parse(None), Actor::Missing);
}

/// A named value, a near miss around it, or any string.
fn around(named: &'static [&'static str]) -> impl Strategy<Value = String> {
    let names = named.to_vec();
    let near = named.to_vec();
    prop_oneof![
        3 => proptest::sample::select(names).prop_map(str::to_string),
        3 => (proptest::sample::select(near), "[ a-zA-Z0-9_.\\[\\]-]{1,3}", any::<bool>())
            .prop_map(|(name, noise, before)| if before {
                format!("{noise}{name}")
            } else {
                format!("{name}{noise}")
            }),
        1 => proptest::sample::select(named.to_vec())
            .prop_map(|name| name.to_ascii_uppercase()),
        2 => any::<String>(),
    ]
}

/// A UTC time, often inside a leap second.
fn time() -> impl Strategy<Value = DateTime<Utc>> {
    prop_oneof![
        (-50_000_000_000i64..50_000_000_000, 0u32..1_000_000_000),
        // A leap second is second 59 with 1e9 or more nanoseconds.
        (
            -833_333_333i64..833_333_333,
            1_000_000_000u32..2_000_000_000
        )
            .prop_map(|(minute, nanos)| (minute * 60 + 59, nanos)),
    ]
    .prop_filter_map("a valid time", |(secs, nanos)| {
        DateTime::<Utc>::from_timestamp(secs, nanos)
    })
}

fn check_run() -> impl Strategy<Value = CheckRun> {
    (
        0u64..4,
        proptest::sample::select(vec!["a", "b", QUEUE_CHECK, QUEUE_RULE_CHECK]),
        proptest::sample::select(vec!["head", "other"]),
        proptest::sample::select(vec![
            None,
            Some((MERGIFY_APP_ID, "mergify")),
            Some((1, "mergify")),
            Some((15368, "github-actions")),
            Some((1, "other")),
        ]),
        proptest::sample::select(vec!["completed", "in_progress", "queued", "weird"]),
        proptest::option::of(proptest::sample::select(vec!["success", "failure", "odd"])),
        proptest::option::of(time()),
    )
        .prop_map(
            |(id, name, head, app, status, conclusion, completed_at)| CheckRun {
                id,
                pull_requests: vec![],
                name: name.into(),
                head_sha: head.into(),
                external_id: None,
                details_url: None,
                status: status.into(),
                conclusion: conclusion.map(Into::into),
                completed_at,
                app: app.map(|(id, slug)| CheckApp {
                    id,
                    slug: slug.into(),
                }),
                output: None,
                check_suite: None,
            },
        )
}

fn time_of(time: DateTime<Utc>) -> Time {
    Time {
        secs: time.timestamp(),
        nanos: time.timestamp_subsec_nanos(),
    }
}

#[test]
fn a_leap_second_orders_between_its_neighbours() {
    let at = |secs, nanos| time_of(DateTime::<Utc>::from_timestamp(secs, nanos).unwrap());
    // 2016-12-31T23:59:59.5Z, the leap second 23:59:60.5Z, then midnight.
    let before = at(1_483_228_799, 500_000_000);
    let leap = at(1_483_228_799, 1_500_000_000);
    let after = at(1_483_228_800, 0);
    assert!(before < leap && leap < after);
}

proptest! {
    #![proptest_config(Config {
        // Integration tests have no lib.rs to anchor a regressions file.
        failure_persistence: None,
        ..Config::default()
    })]

    #[test]
    fn status_parse_is_total(value in around(&STATUS_NAMES)) {
        let status = Status::parse(&value);
        match status.name() {
            Some(name) => prop_assert_eq!(name, value.as_str()),
            None => prop_assert!(!STATUS_NAMES.contains(&value.as_str())),
        }
    }

    #[test]
    fn conclusion_parse_is_total(value in around(&CONCLUSION_NAMES)) {
        let conclusion = Conclusion::parse(&value);
        match conclusion.name() {
            Some(name) => prop_assert_eq!(name, value.as_str()),
            None => prop_assert!(!CONCLUSION_NAMES.contains(&value.as_str())),
        }
    }

    #[test]
    fn app_parse_is_total(
        id in prop_oneof![Just(MERGIFY_APP_ID), Just(MERGIFY_APP_ID + 1), any::<u64>()],
        slug in around(&["mergify", "github-actions"]),
    ) {
        let app = App::parse(Some((id, &slug)));
        prop_assert_eq!(app == App::Mergify, id == MERGIFY_APP_ID && slug == "mergify");
        prop_assert_eq!(app.mergify_slug(), slug == "mergify");
        prop_assert_eq!(app == App::Actions, slug == "github-actions");
        prop_assert!(app != App::Missing);
    }

    #[test]
    fn name_parse_is_total(name in around(&[QUEUE_CHECK, QUEUE_RULE_CHECK])) {
        let expected = if name == QUEUE_CHECK {
            Name::QueueCheck
        } else if name == QUEUE_RULE_CHECK {
            Name::QueueRule
        } else {
            Name::Other
        };
        prop_assert_eq!(Name::parse(&name), expected);
    }

    #[test]
    fn actor_parse_is_total(login in around(&["mergify[bot]", "RobTand", "[bot]", "bot"])) {
        let actor = Actor::parse(Some(&login));
        prop_assert_eq!(actor == Actor::Bot, login.ends_with("[bot]"));
        prop_assert!(actor != Actor::Missing);
    }

    #[test]
    fn mergeable_parse_is_total(state in around(&["unknown", "dirty", "clean"])) {
        let parsed = Mergeable::parse(Some(&state));
        let expected = match state.as_str() {
            "unknown" => Mergeable::Unknown,
            "dirty" => Mergeable::Dirty,
            _ => Mergeable::Other,
        };
        prop_assert_eq!(parsed, expected);
    }

    /// `views` interns names and app IDs per read, compares heads exactly and
    /// keeps chrono's time order, leap seconds included.
    #[test]
    fn views_keep_identity_head_and_time_order(
        checks in proptest::collection::vec(check_run(), 0..6),
        head in proptest::sample::select(vec!["head", "other", "neither"]),
    ) {
        let views = tl3_mergify::views(&checks, head, |check| check.name == "a");
        prop_assert_eq!(views.len(), checks.len());
        for (i, (view, check)) in views.iter().zip(&checks).enumerate() {
            prop_assert_eq!(view.id, check.id);
            prop_assert_eq!(view.head == Head::Current, check.head_sha == head);
            prop_assert_eq!(view.name, Name::parse(&check.name));
            prop_assert_eq!(
                view.app,
                App::parse(check.app.as_ref().map(|app| (app.id, app.slug.as_str())))
            );
            prop_assert_eq!(view.required, check.name == "a");
            prop_assert_eq!(view.status, Status::parse(&check.status));
            prop_assert_eq!(view.conclusion, check.conclusion.as_deref().map(Conclusion::parse));
            prop_assert_eq!(view.completed_at, check.completed_at.map(time_of));
            for (other_view, other) in views.iter().zip(&checks).skip(i + 1) {
                prop_assert_eq!(view.name_key == other_view.name_key, check.name == other.name);
                prop_assert_eq!(
                    view.app_key == other_view.app_key,
                    check.app.as_ref().map(|app| app.id) == other.app.as_ref().map(|app| app.id)
                );
                prop_assert_eq!(
                    view.completed_at.cmp(&other_view.completed_at),
                    check.completed_at.cmp(&other.completed_at)
                );
            }
        }
    }
}
