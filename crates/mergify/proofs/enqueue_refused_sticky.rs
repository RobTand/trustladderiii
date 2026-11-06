//! Proof root over the actual enqueue state machine and its real Vec effects.
#[macro_use]
extern crate verus_builtin_macros;
#[macro_use]
#[path = "../src/decide/verification.rs"]
mod verification;
#[path = "../src/decide/check.rs"]
mod check;
#[path = "../src/decide/observe.rs"]
mod observe;
#[path = "../src/decide/enqueue.rs"]
mod enqueue;
use check::{App, CheckView, Conclusion, Head, Status, Time};
use observe::QueueState;
const MERGIFY_APP_ID: u64 = 10562;
const MERGIFY_APP_SLUG: &str = "mergify";
const QUEUE_CHECK: &str = "Mergify Merge Queue";
const QUEUE_RULE_CHECK: &str = "Rule: Queue TL3-approved PRs (queue)";
mod enqueue_helpers;
#[path = "enqueue_refused_sticky_spec.rs"]
mod enqueue_contract;
#[path = "enqueue_refused_sticky_witness.rs"]
mod witness;
fn main() {}
