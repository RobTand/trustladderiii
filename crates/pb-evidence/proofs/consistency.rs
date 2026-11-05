use vstd::prelude::*;
#[path = "../src/decision.rs"]
mod decision;
verus! {
/// #821 / Q55: accepted and each refusal are reachable within all 2^8 typed states.
fn consistency_witness() {
    let value = decision::consistency(true, true, true, true, true, true, true, true);
    assert(value == 0);
    let value = decision::consistency(true, true, false, true, false, false, true, false);
    assert(value == 1);
    let value = decision::consistency(true, true, true, true, false, true, true, false);
    assert(value == 2);
    let value = decision::consistency(true, true, true, true, true, true, true, false);
    assert(value == 3);
}
}
fn main() {}
