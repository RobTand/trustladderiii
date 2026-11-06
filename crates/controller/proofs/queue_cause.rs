//! #854b / Q50/Q51: all scalar inputs, no unwind bound or sequence truncation.
#![allow(dead_code)]
#[path = "../src/queue_cause_decide.rs"]
mod queue_cause_decide;
use queue_cause_decide::*;
use vstd::prelude::*;
verus! {
fn attribution_witness() {
    let opaque = attribute(false, false, false); assert(opaque == Attribution::Unattributed);
    let changed = attribute(true, false, false); assert(changed == Attribution::Change);
    let environment = attribute(false, true, false); assert(environment == Attribution::Environment);
    let second = attribute(false, true, true); assert(second == Attribution::Change);
}
fn bounded_hold_witness() {
    let wait = hold(false, false, false); assert(wait == HoldAction::Wait);
    let requeue = hold(true, false, false); assert(requeue == HoldAction::Requeue);
    let second = hold(true, true, false); assert(second != HoldAction::Requeue);
    let bounded = hold(false, false, true); assert(bounded == HoldAction::Incident);
    let after_retry = hold(true, true, true); assert(after_retry == HoldAction::Incident);
}
}
fn main() {}
