//! The same assertions on enumerated and sampled inputs.
extern crate alloc;
#[path = "kani_scenarios/common/mod.rs"]
mod common;
use common::*;

#[test]
fn kani_gate_leaf_progress_w2() { common::gate_leaf(2); }

#[test]
fn kani_gate_leaf_progress_w3() { common::gate_leaf(3); }
