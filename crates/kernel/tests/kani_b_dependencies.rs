//! The same assertions on enumerated and sampled inputs.
extern crate alloc;
#[path = "kani_scenarios/common/mod.rs"]
mod common;
use common::*;

#[test]
fn kani_apply_dependency_registration() { common::dependency_registration(); }

#[test]
fn kani_apply_dependency_remove() { common::dependency_remove(); }

#[test]
fn kani_dependency_no_cycle() { common::dependency_cycles(); }

#[test]
fn kani_dependency_waiting_holds_nothing() { common::dependency_waiting(false); }

#[test]
fn kani_dependency_delivered_releases() { common::dependency_waiting(true); }
