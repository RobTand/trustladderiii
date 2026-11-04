//! Boundary parsers: GitHub's strings into the queue decisions' enums (#735).
//! Every named value round-trips; any other string is `Other`, never a
//! named value; parsing never panics.

use proptest::prelude::*;
use tl3_mergify::decide::facts::Actor;
use tl3_mergify::decide::observe::Mergeable;
use tl3_mergify::decide::{
    App, Conclusion, MERGIFY_APP_ID, Name, QUEUE_CHECK, QUEUE_RULE_CHECK, Status,
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

proptest! {
    #[test]
    fn status_parse_is_total(value in any::<String>()) {
        let status = Status::parse(&value);
        match status.name() {
            Some(name) => prop_assert_eq!(name, value.as_str()),
            None => prop_assert!(!STATUS_NAMES.contains(&value.as_str())),
        }
    }

    #[test]
    fn conclusion_parse_is_total(value in any::<String>()) {
        let conclusion = Conclusion::parse(&value);
        match conclusion.name() {
            Some(name) => prop_assert_eq!(name, value.as_str()),
            None => prop_assert!(!CONCLUSION_NAMES.contains(&value.as_str())),
        }
    }

    #[test]
    fn named_strings_with_noise_are_other(index in 0..CONCLUSION_NAMES.len(), noise in "[ A-Z_.-]{1,3}") {
        let name = CONCLUSION_NAMES[index];
        prop_assert_eq!(Conclusion::parse(&format!("{name}{noise}")), Conclusion::Other);
        prop_assert_eq!(Conclusion::parse(&format!("{noise}{name}")), Conclusion::Other);
        let status = STATUS_NAMES[index % STATUS_NAMES.len()];
        prop_assert_eq!(Status::parse(&format!("{status}{noise}")), Status::Other);
    }

    #[test]
    fn app_parse_is_total(id in any::<u64>(), slug in any::<String>()) {
        let app = App::parse(Some((id, &slug)));
        prop_assert_eq!(app == App::Mergify, id == MERGIFY_APP_ID && slug == "mergify");
        prop_assert_eq!(app.mergify_slug(), slug == "mergify");
        prop_assert_eq!(app == App::Actions, slug == "github-actions");
        prop_assert!(app != App::Missing);
    }

    #[test]
    fn name_parse_is_total(name in any::<String>()) {
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
    fn actor_parse_is_total(login in any::<String>()) {
        let actor = Actor::parse(Some(&login));
        prop_assert_eq!(actor == Actor::Bot, login.ends_with("[bot]"));
        prop_assert!(actor != Actor::Missing);
    }

    #[test]
    fn mergeable_parse_is_total(state in any::<String>()) {
        let parsed = Mergeable::parse(Some(&state));
        let expected = match state.as_str() {
            "unknown" => Mergeable::Unknown,
            "dirty" => Mergeable::Dirty,
            _ => Mergeable::Other,
        };
        prop_assert_eq!(parsed, expected);
    }
}
