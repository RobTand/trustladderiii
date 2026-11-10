//! The same assertions on enumerated and sampled inputs.
extern crate alloc;
#[path = "kani_scenarios/common/mod.rs"]
mod common;
use common::*;
#[path = "kani_scenarios/common/a_extra.rs"]
mod a_extra;

#[test]
fn kani_advance_ingesting() {
    a_extra::run_advance(44, "kani_advance_ingesting");
}

#[test]
fn kani_advance_auditing() {
    a_extra::run_advance(45, "kani_advance_auditing");
}

#[test]
fn kani_advance_unplanned() {
    a_extra::run_advance(46, "kani_advance_unplanned");
}

#[test]
fn kani_advance_planning() {
    a_extra::run_advance(47, "kani_advance_planning");
}

#[test]
fn kani_advance_ready() {
    a_extra::run_advance(48, "kani_advance_ready");
}

#[test]
fn kani_advance_making() {
    a_extra::run_advance(49, "kani_advance_making");
}

#[test]
fn kani_advance_checking() {
    a_extra::run_advance(50, "kani_advance_checking");
}

#[test]
fn kani_advance_reviewing() {
    a_extra::run_advance(51, "kani_advance_reviewing");
}

#[test]
fn kani_advance_publishing() {
    a_extra::run_advance(52, "kani_advance_publishing");
}

#[test]
fn kani_advance_awaiting_delivery() {
    a_extra::run_advance(53, "kani_advance_awaiting_delivery");
}

#[test]
fn kani_advance_rebasing() {
    a_extra::run_advance(54, "kani_advance_rebasing");
}

#[test]
fn kani_advance_stopping() {
    a_extra::run_advance(55, "kani_advance_stopping");
}

