//! The same assertions on enumerated and sampled inputs.
#[path = "kani_scenarios/common/mod.rs"]
mod common;
use common::*;

#[test]
fn kani_pool_grant_is_priority_ordered() { common::pool_grant(); }

#[test]
fn kani_pool_order_is_strict_priority() { common::pool_strict(); }
