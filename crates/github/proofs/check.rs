#[macro_use]
#[path = "../src/checks/verification.rs"]
mod verification;
#[path = "../src/checks/check.rs"]
mod check;
/// The check run Mergify's GitHub App posts for a queued PR.
pub const QUEUE_CHECK: &str = "Mergify Merge Queue";
/// The check run for TL3's queue rule.
pub const QUEUE_RULE_CHECK: &str = "Rule: Queue TL3-approved PRs (queue)";
pub const MERGIFY_APP_ID: u64 = 10562;
pub const MERGIFY_APP_SLUG: &str = "mergify";
fn main() {}
