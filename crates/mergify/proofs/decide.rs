//! #743 M1b: import the real decision functions and executable covers.
#[macro_use]
#[path = "../src/decide/verification.rs"]
mod verification;
#[path = "../src/decide/check.rs"]
mod check;
#[path = "../src/decide/observe.rs"]
mod observe;
const MERGIFY_APP_ID: u64 = 10562;
const MERGIFY_APP_SLUG: &str = "mergify";
const QUEUE_CHECK: &str = "Mergify Merge Queue";
const QUEUE_RULE_CHECK: &str = "Rule: Queue TL3-approved PRs (queue)";
#[path = "../src/decide/facts.rs"]
mod facts;
#[path = "../src/decide/translate.rs"]
mod translate;
#[path = "../src/decide/decide_witness.rs"]
mod decide_witness;
mod decide_properties;
fn main() {}
