//! Check runs as small enums. The adapter parses GitHub's strings into these
//! at its boundary; every decision in this crate reads only the enums.

use super::{MERGIFY_APP_ID, MERGIFY_APP_SLUG, QUEUE_CHECK, QUEUE_RULE_CHECK};

/// A check run's or check suite's `status`.
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
    pub fn passing(self) -> bool {
        matches!(self, Self::Success | Self::Skipped | Self::Neutral)
    }
}

/// Whether a check run is on the head the caller reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Head {
    Current,
    Other,
}

/// The GitHub App that posted a check run.
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
    pub fn mergify_slug(self) -> bool {
        matches!(self, Self::Mergify | Self::MergifySlug)
    }
}

/// The check names Mergify's decisions single out.
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Time {
    pub secs: i64,
    pub nanos: u32,
}

/// One check run, parsed. Keys are interned by the adapter: equal keys mean
/// equal strings or app IDs within one read.
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
    pub fn completed(&self) -> bool {
        self.status == Status::Completed
    }
}

/// Whether `checks[i]` is a member that a `BTreeMap` keyed by group keeps,
/// filled in order with `entry(key).or_insert(check)` and replaced only by a
/// strictly larger ID: the first member with the largest ID in its group.
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
    while j < checks.len() {
        let other = &checks[j];
        if j != i
            && member(other)
            && same(other, check)
            && (other.id > check.id || j < i && other.id == check.id)
        {
            return false;
        }
        j += 1;
    }
    true
}
