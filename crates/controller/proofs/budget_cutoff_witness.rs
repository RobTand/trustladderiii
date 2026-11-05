#[path = "../src/budget/cutoff.rs"]
mod cutoff;
use vstd::prelude::*;
verus! {
/// Q4/#718/#743: both original cutoff covers execute the real full-width function.
fn cutoff_witness(case: bool) -> (result: bool)
    ensures result,
{
    let actual = cutoff::cutoff(if case { 17 } else { 16 }, 20);
    if case { actual } else { !actual }
}
}
fn main() {}
