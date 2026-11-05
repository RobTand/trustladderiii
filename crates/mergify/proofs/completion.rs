//! A1 proofs use the actual production types and executable completion module.
//! Scalar inputs span their full domains; loops use induction over any finite slice.
#![allow(dead_code)]
const MERGIFY_APP_ID: u64 = 10562;
const MERGIFY_APP_SLUG: &str = "mergify";
const QUEUE_CHECK: &str = "Mergify Merge Queue";
const QUEUE_RULE_CHECK: &str = "Rule: Queue TL3-approved PRs (queue)";
#[path = "../src/decide/check.rs"]
mod check;
#[path = "../src/decide/observe.rs"]
mod observe;
use check::{Head, Time};
use observe::QueueState;
#[path = "../src/decide/completion.rs"]
mod completion;
mod completion_spec;
fn main() {}
