//! Check runs as small enums. The adapter parses GitHub's strings into these
//! at its boundary; every decision in this crate reads only the enums.

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

use super::{MERGIFY_APP_ID, MERGIFY_APP_SLUG, QUEUE_CHECK, QUEUE_RULE_CHECK};

/// A check run's or check suite's `status`.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Status {
    Queued,
    InProgress,
    Completed,
    Waiting,
    Requested,
    Pending,
    /// Any value GitHub may add later.
    Other,
}

impl Status {
    pub const NAMED: [Status; 6] = [
        Status::Queued,
        Status::InProgress,
        Status::Completed,
        Status::Waiting,
        Status::Requested,
        Status::Pending,
    ];

    pub fn parse(value: &str) -> Self {
        match value {
            "queued" => Self::Queued,
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            "waiting" => Self::Waiting,
            "requested" => Self::Requested,
            "pending" => Self::Pending,
            _ => Self::Other,
        }
    }

    /// GitHub's name for the status; `None` for `Other`.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::Queued => "queued",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Waiting => "waiting",
            Self::Requested => "requested",
            Self::Pending => "pending",
            Self::Other => return None,
        })
    }
}

/// A check run's `conclusion`. A missing conclusion is `Option::None`.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Conclusion {
    Success,
    Failure,
    Neutral,
    Cancelled,
    Skipped,
    TimedOut,
    ActionRequired,
    StartupFailure,
    Stale,
    /// Any value GitHub may add later. It still counts as concluded.
    Other,
}

impl Conclusion {
    pub const NAMED: [Conclusion; 9] = [
        Conclusion::Success,
        Conclusion::Failure,
        Conclusion::Neutral,
        Conclusion::Cancelled,
        Conclusion::Skipped,
        Conclusion::TimedOut,
        Conclusion::ActionRequired,
        Conclusion::StartupFailure,
        Conclusion::Stale,
    ];

    pub fn parse(value: &str) -> Self {
        match value {
            "success" => Self::Success,
            "failure" => Self::Failure,
            "neutral" => Self::Neutral,
            "cancelled" => Self::Cancelled,
            "skipped" => Self::Skipped,
            "timed_out" => Self::TimedOut,
            "action_required" => Self::ActionRequired,
            "startup_failure" => Self::StartupFailure,
            "stale" => Self::Stale,
            _ => Self::Other,
        }
    }

    /// GitHub's name for the conclusion; `None` for `Other`.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Neutral => "neutral",
            Self::Cancelled => "cancelled",
            Self::Skipped => "skipped",
            Self::TimedOut => "timed_out",
            Self::ActionRequired => "action_required",
            Self::StartupFailure => "startup_failure",
            Self::Stale => "stale",
            Self::Other => return None,
        })
    }

    /// A conclusion that lets a merge or a required check pass.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
        ensures result == matches!(self, Self::Success | Self::Skipped | Self::Neutral),
    ))]
    pub fn passing(self) -> bool {
        matches!(self, Self::Success | Self::Skipped | Self::Neutral)
    }
}

/// Whether a check run is on the head the caller reads.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Head {
    Current,
    Other,
}

/// The GitHub App that posted a check run.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum App {
    /// Mergify's own app: its ID and slug both match.
    Mergify,
    /// The `mergify` slug with another app ID.
    MergifySlug,
    /// The `github-actions` app.
    Actions,
    Other,
    /// The check run names no app.
    Missing,
}

impl App {
    pub fn parse(app: Option<(u64, &str)>) -> Self {
        match app {
            None => Self::Missing,
            Some((id, MERGIFY_APP_SLUG)) if id == MERGIFY_APP_ID => Self::Mergify,
            Some((_, MERGIFY_APP_SLUG)) => Self::MergifySlug,
            Some((_, "github-actions")) => Self::Actions,
            Some(_) => Self::Other,
        }
    }

    /// The slug is `mergify`, whatever the app ID.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
        ensures result == matches!(self, Self::Mergify | Self::MergifySlug),
    ))]
    pub fn mergify_slug(self) -> bool {
        matches!(self, Self::Mergify | Self::MergifySlug)
    }
}

/// The check names Mergify's decisions single out.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Name {
    QueueCheck,
    QueueRule,
    Other,
}

impl Name {
    pub fn parse(name: &str) -> Self {
        match name {
            QUEUE_CHECK => Self::QueueCheck,
            QUEUE_RULE_CHECK => Self::QueueRule,
            _ => Self::Other,
        }
    }
}

/// A UTC time as Unix seconds plus nanoseconds, ordered as chrono orders it.
/// Leap seconds carry nanoseconds of 1e9 or more.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Time {
    pub secs: i64,
    pub nanos: u32,
}

/// One check run, parsed. Keys are interned by the adapter: equal keys mean
/// equal strings or app IDs within one read.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct CheckView {
    pub id: u64,
    pub head: Head,
    pub app: App,
    /// The interned `Option<app id>`.
    pub app_key: u32,
    pub name: Name,
    /// The interned check name.
    pub name_key: u32,
    /// The check matches one of the caller's required contexts.
    pub required: bool,
    pub status: Status,
    pub conclusion: Option<Conclusion>,
    pub completed_at: Option<Time>,
}

impl CheckView {
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
        ensures result == (self.status == Status::Completed),
    ))]
    pub fn completed(&self) -> bool {
        self.status == Status::Completed
    }
}

/// Whether `checks[i]` is a member that a `BTreeMap` keyed by group keeps,
/// filled in order with `entry(key).or_insert(check)` and replaced only by a
/// strictly larger ID: the first member with the largest ID in its group.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    requires callbacks(checks@, member, same),
    ensures forall|by_app: bool| grouping(checks@, member, same, by_app) ==>
        result == kept(checks@, i as int, by_app),
))]
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
    decide_while! { (j < checks.len()) [
        invariant j <= checks@.len(), i < checks@.len(), *check == checks@[i as int],
            callbacks(checks@, member, same),
            member.ensures((check,), true),
            forall|by_app: bool| grouping(checks@, member, same, by_app) ==>
                kept_prefix(checks@, i as int, j as int, by_app),
        decreases checks@.len() - j,
    ] {
        let other = &checks[j];
        decide_proof! { assert(same.requires((other, check))); }
        if j != i && member(other) && same(other, check)
            && (other.id > check.id || j < i && other.id == check.id)
        {
            decide_proof! {
                assert forall|by_app: bool| grouping(checks@, member, same, by_app)
                    implies !kept(checks@, i as int, by_app) by {
                    assert(member.ensures((other,), true));
                    assert(same.ensures((other, check), true));
                    assert(other.head == Head::Current);
                    assert(other.name_key == check.name_key && (!by_app || other.app_key == check.app_key));
                }
            }
            return false;
        }
        decide_proof! {
            assert forall|by_app: bool| grouping(checks@, member, same, by_app)
                implies kept_prefix(checks@, i as int, j as int + 1, by_app) by {
                assert(kept_prefix(checks@, i as int, j as int, by_app));
                assert(other.head != Head::Current
                    || other.name_key != check.name_key
                    || by_app && other.app_key != check.app_key
                    || other.id < check.id || other.id == check.id && j >= i);
            }
        }
        j += 1;
    } }
    true
}

#[cfg(verus_keep_ghost)]
verus! {
/// #743: callbacks are callable on every possible list member.
pub open spec fn callbacks(c: Seq<CheckView>, member: impl Fn(&CheckView) -> bool,
    same: impl Fn(&CheckView, &CheckView) -> bool) -> bool {
    (forall|j: int| 0 <= j < c.len() ==> #[trigger] member.requires((&c[j],)))
    && (forall|j: int, k: int| 0 <= j < c.len() && 0 <= k < c.len() ==>
        #[trigger] same.requires((&c[j], &c[k])))
}
/// #743: callback postconditions identify the original two BTreeMap key choices.
pub open spec fn grouping(c: Seq<CheckView>, member: impl Fn(&CheckView) -> bool,
    same: impl Fn(&CheckView, &CheckView) -> bool, by_app: bool) -> bool {
    (forall|j: int, r: bool| 0 <= j < c.len() && member.ensures((&c[j],), r) ==>
        r == (c[j].head == Head::Current))
    && (forall|j: int, k: int, r: bool| 0 <= j < c.len() && 0 <= k < c.len()
        && same.ensures((&c[j], &c[k]), r) ==>
        r == (c[j].name_key == c[k].name_key && (!by_app || c[j].app_key == c[k].app_key)))
}
/// #743: a candidate is the first maximum against the already scanned prefix.
pub open spec fn kept_prefix(c: Seq<CheckView>, i: int, n: int, by_app: bool) -> bool {
    0 <= i < c.len() && c[i].head == Head::Current
        && forall|j: int| 0 <= j < n && c[j].head == Head::Current
            && c[j].name_key == c[i].name_key && (!by_app || c[j].app_key == c[i].app_key) ==>
            c[j].id < c[i].id || c[j].id == c[i].id && j >= i
}
/// #743: BTreeMap entry/or_insert retains the first maximum by app/name or name.
pub open spec fn kept(c: Seq<CheckView>, i: int, by_app: bool) -> bool {
    kept_prefix(c, i, c.len() as int, by_app)
}
}
