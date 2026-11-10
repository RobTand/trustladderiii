//! The same assertions on enumerated and sampled inputs.
extern crate alloc;
#[path = "kani_scenarios/common/mod.rs"]
mod common;
use common::*;

#[test]
fn kani_apply_adoption_at_most_once() { common::adoption(false); }

#[test]
fn kani_apply_adoption_has_no_maker_credit() { common::adoption(true); }
