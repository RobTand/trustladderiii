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

fn queue_payload(payload: serde_json::Value) -> CheckRun {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    serde_json::from_value(serde_json::json!({
        "id": 1, "name": QUEUE_CHECK, "head_sha": "head", "status": "completed",
        "conclusion": "neutral", "output": {"summary": format!(
            "<span itemprop=\"mergify.v1.{}\"></span>",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
        )}
    }))
    .unwrap()
}

fn queue_conditions(nodes: serde_json::Value) -> CheckRun {
    queue_payload(serde_json::json!({"version":1,"state":"waiting","required_conditions":nodes}))
}

/// #854 r1: boundary identities round-trip through arbitrarily satisfied groups.
proptest! {
    #![proptest_config(Config { failure_persistence: None, ..Config::default() })]
    #[test]
    fn queue_check_identity_round_trips(
        name in "[A-Za-z_][A-Za-z0-9_ /.-]{0,48}",
        app in proptest::option::of("[a-z][a-z0-9-]{0,20}"),
        groups in proptest::collection::vec(any::<bool>(), 0..9),
        matched in any::<bool>(),
    ) {
        use tl3_mergify::QueueEntryCondition;
        let identity = app.as_ref().map_or_else(|| name.clone(), |app| format!("@{app}/{name}"));
        let mut node = serde_json::json!({"description":format!("check-success = {identity}"), "match":matched});
        for (index, matched) in groups.iter().enumerate() {
            node = serde_json::json!({"description":if index % 2 == 0 {"all of"} else {"any of"}, "match":matched, "subconditions":[node]});
        }
        let parsed = tl3_mergify::queue_entry_conditions(&queue_conditions(serde_json::json!([node])));
        let expected = if matched || groups.iter().any(|matched| *matched) {
            vec![]
        } else {
            vec![QueueEntryCondition::Check { name, app }]
        };
        prop_assert_eq!(parsed, expected);
    }
}

/// #854 r1: app-qualified identities never match a same-named check from another app.
#[test]
fn queue_attribution_keeps_app_identity_and_display_text() {
    use serde_json::json;
    let queue = queue_conditions(json!([
        {"description":"scopes-available","match":true},
        {"description":"all of","match":false,"note":"  blocked  ","subconditions":[
            {"description":"any of","match":false,"subconditions":[
                {"description":"check-success = @github-actions/Bazel","match":false}
            ]}
        ]}
    ]));
    assert_eq!(tl3_mergify::unmet_conditions(&queue), ["all of: blocked"]);
    let parsed = tl3_mergify::queue_entry_conditions(&queue);
    assert_eq!(parsed.len(), 1);
    let mut check = queue.clone();
    check.name = "Bazel".into();
    check.app = Some(CheckApp {
        id: 15368,
        slug: "github-actions".into(),
    });
    assert!(parsed[0].matches(&check));
    check.app.as_mut().unwrap().slug = "other-app".into();
    assert!(!parsed[0].matches(&check));
    check.app = None;
    assert!(!parsed[0].matches(&check));
    let unqualified =
        queue_conditions(json!([{"description":"check-neutral = Bazel","match":false}]));
    assert!(tl3_mergify::queue_entry_conditions(&unqualified)[0].matches(&check));
    let scopes = queue_conditions(json!([{"description":"scopes-available","match":false}]));
    assert!(tl3_mergify::queue_entry_conditions(&scopes)[0].matches(&check));
}

/// #854 r1: malformed/unknown nodes must not acquire a child's attribution.
#[test]
fn queue_attribution_ignores_unknown_or_malformed_nodes() {
    use serde_json::json;
    let leaf = json!({"description":"check-success = @github-actions/Bazel","match":false});
    for node in [
        json!(null),
        json!({"description":"all of","subconditions":[leaf]}),
        json!({"description":"all of","match":"false","subconditions":[leaf]}),
        json!({"description":"unknown group","match":false,"subconditions":[leaf]}),
        json!({"description":"all of","match":false,"subconditions":{}}),
        json!({"description":"any of","match":true,"subconditions":[leaf]}),
        json!({"description":"check-success = @/Bazel","match":false}),
        json!({"description":"check-success = @github-actions","match":false}),
        json!({"description":"check-success = ","match":false}),
        json!({"description":"check-success = Bazel","match":false,"subconditions":[leaf]}),
    ] {
        assert!(tl3_mergify::queue_entry_conditions(&queue_conditions(json!([node]))).is_empty());
    }
    for payload in [
        json!({"version":2,"state":"waiting","required_conditions":[leaf]}),
        json!({"version":1,"state":"unknown","required_conditions":[leaf]}),
        json!({"version":1,"state":"waiting","required_conditions":{}}),
        json!({"version":1,"state":"waiting"}),
    ] {
        assert!(tl3_mergify::queue_entry_conditions(&queue_payload(payload)).is_empty());
    }
    let mut queue = queue_conditions(json!([leaf]));
    queue.output.as_mut().unwrap().summary =
        Some("<span itemprop=\"mergify.v1.invalid!\"></span>".into());
    assert!(tl3_mergify::queue_entry_conditions(&queue).is_empty());
}

/// #854 r1: oversized input, too many nodes and deep JSON fail closed without panicking.
#[test]
fn queue_attribution_has_byte_node_and_depth_bounds() {
    use serde_json::json;
    let leaf = json!({"description":"check-skipped = Bazel","match":false});
    assert_eq!(
        tl3_mergify::queue_entry_conditions(&queue_conditions(json!(vec![leaf.clone(); 256])))
            .len(),
        256
    );
    assert!(
        tl3_mergify::queue_entry_conditions(&queue_conditions(json!(vec![leaf.clone(); 257])))
            .is_empty()
    );
    for depth in [16, 17, 140] {
        let mut node = leaf.clone();
        for _ in 0..depth {
            node = json!({"description":"all of","match":false,"subconditions":[node]});
        }
        let parsed = tl3_mergify::queue_entry_conditions(&queue_conditions(json!([node])));
        assert_eq!(parsed.len(), usize::from(depth == 16), "depth {depth}");
    }
    let mut oversized = queue_conditions(json!([leaf]));
    oversized
        .output
        .as_mut()
        .unwrap()
        .summary
        .as_mut()
        .unwrap()
        .push_str(&" ".repeat(256 * 1024));
    assert!(tl3_mergify::queue_entry_conditions(&oversized).is_empty());
}
