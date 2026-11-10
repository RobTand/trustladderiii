//! The same assertions on enumerated and sampled inputs.
#[path = "kani_scenarios/common/mod.rs"]
mod common;
use common::*;

#[test]
fn kani_fail_fast_plan_cant_meet() { common::fail_fast_divert(0); }

#[test]
fn kani_fail_fast_maker_needs_files() { common::fail_fast_divert(1); }

#[test]
fn kani_fail_fast_maker_cant_meet() { common::fail_fast_divert(2); }

#[test]
fn kani_fail_fast_maker_out_of_room() { common::fail_fast_divert(3); }

#[test]
fn kani_fail_fast_checks_done() { common::fail_fast_divert(4); }

#[test]
fn kani_fail_fast_changes_requested() { common::fail_fast_divert(5); }

#[test]
fn kani_fail_fast_reviewer_out_of_room() { common::fail_fast_divert(6); }

#[test]
fn kani_fail_fast_ci_failed_awaiting_delivery() { common::fail_fast_divert(7); }

#[test]
fn kani_fail_fast_ci_failed_publishing() { common::fail_fast_divert(8); }
