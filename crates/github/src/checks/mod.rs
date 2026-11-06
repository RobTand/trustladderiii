//! Typed check views and required-check evaluation.
#[macro_use]
mod verification;

pub mod check;
pub mod required;
pub use check::{App, CheckView, Conclusion, Head, Name, Status, Time};

use crate::CheckRun;
use std::collections::BTreeMap;

/// The check run Mergify's GitHub App posts for a queued PR.
pub const QUEUE_CHECK: &str = "Mergify Merge Queue";
/// The check run for TL3's queue rule.
pub const QUEUE_RULE_CHECK: &str = "Rule: Queue TL3-approved PRs (queue)";
pub const MERGIFY_APP_ID: u64 = 10562;
pub const MERGIFY_APP_SLUG: &str = "mergify";

/// Parse check runs into the core's views at the adapter boundary. `head` is
/// the commit the caller reads; `required` marks the caller's required
/// contexts. Names and app IDs are interned per call.
pub fn views(
    checks: &[CheckRun],
    head: &str,
    required: impl Fn(&CheckRun) -> bool,
) -> Vec<CheckView> {
    let mut names = BTreeMap::new();
    let mut apps = BTreeMap::new();
    checks
        .iter()
        .map(|check| {
            let next = names.len() as u32;
            let name_key = *names.entry(check.name.as_str()).or_insert(next);
            let next = apps.len() as u32;
            let app_key = *apps
                .entry(check.app.as_ref().map(|app| app.id))
                .or_insert(next);
            CheckView {
                id: check.id,
                head: if check.head_sha == head {
                    Head::Current
                } else {
                    Head::Other
                },
                app: App::parse(check.app.as_ref().map(|app| (app.id, app.slug.as_str()))),
                app_key,
                name: Name::parse(&check.name),
                name_key,
                required: required(check),
                status: Status::parse(&check.status),
                conclusion: check.conclusion.as_deref().map(Conclusion::parse),
                completed_at: check.completed_at.map(|time| Time {
                    secs: time.timestamp(),
                    nanos: time.timestamp_subsec_nanos(),
                }),
            }
        })
        .collect()
}
