//! The same assertions on enumerated and sampled inputs beyond Kani bounds.
extern crate alloc;
#[path = "kani_scenarios/common/mod.rs"]
mod common;
use common::*;

#[test]
fn kani_beyond_dependency_registration() { common::b_beyond::dependencies(0); }

#[test]
fn kani_beyond_dependency_remove() { common::b_beyond::dependencies(1); }

#[test]
fn kani_beyond_dependency_cycles() { common::b_beyond::dependencies(2); }

#[test]
fn kani_beyond_dependency_waiting() { common::b_beyond::dependencies(3); }

#[test]
fn kani_beyond_dependency_delivery() { common::b_beyond::dependencies(4); }

#[test]
fn kani_beyond_adoption() { common::b_beyond::adoption(); }

#[test]
fn kani_beyond_gate_grant() { common::b_beyond::gate(false); }

#[test]
fn kani_beyond_gate_planning() { common::b_beyond::gate(true); }

#[test]
fn kani_beyond_rebase_gate() { common::b_beyond::rebase_gate(); }

#[test]
fn kani_beyond_publishing_ci() { common::b_beyond::publishing(false); }

#[test]
fn kani_beyond_fail_fast() { common::b_beyond::fail_fast(); }

#[test]
fn kani_beyond_publishing_earlier() { common::b_beyond::publishing(true); }

#[test]
fn kani_beyond_gate_leaf() { common::b_beyond::gate_leaf(); }

#[test]
fn kani_beyond_pool_grant() { common::b_beyond::pool(); }

#[test]
fn kani_beyond_pool_order() { common::b_beyond::pool_order(); }
