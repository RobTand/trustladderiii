//! The same assertions on enumerated and sampled inputs.
extern crate alloc;
#[path = "kani_scenarios/common/mod.rs"]
mod common;
use common::*;

#[test]
fn kani_apply_rebase_progress_gate() { common::rebase_gate(); }

#[test]
fn kani_apply_publishing_ci_tests() { common::publishing(false); }

#[test]
fn kani_apply_publishing_ci_earlier_step() { common::publishing(true); }
