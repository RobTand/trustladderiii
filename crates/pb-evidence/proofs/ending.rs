use vstd::prelude::*;
#[path = "../src/decision.rs"]
mod decision;
verus! {
/// #821 / Q55: every checked group is necessary, all 2^3 states; ignored metadata is excluded.
fn corruption_and_witness() {
    let value = decision::ending(true, true, true);
    assert(value == 0);
    let value = decision::ending(false, true, true);
    assert(value != 0);
    let value = decision::ending(true, false, true);
    assert(value != 0);
    let value = decision::ending(true, true, false);
    assert(value != 0);
}
}
fn main() {}
