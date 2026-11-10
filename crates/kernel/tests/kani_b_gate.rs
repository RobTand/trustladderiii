//! The same assertions on enumerated and sampled inputs.
#[path = "kani_scenarios/common/mod.rs"]
mod common;
use common::*;

#[test]
fn kani_gate_grant_w2() { common::gate(2, false); }

#[test]
fn kani_gate_grant_w3() { common::gate(3, false); }

#[test]
fn kani_gate_planning_w2() { common::gate(2, true); }

#[test]
fn kani_gate_planning_w3() { common::gate(3, true); }
