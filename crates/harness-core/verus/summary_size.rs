//! #823: the production serializer's per-code-point and budget arithmetic.
//! Unbounded induction over serializer atoms; no finite string-length assumption.
//! Witnesses cover ASCII, controls, BMP, non-BMP, and an isolated surrogate.

#[path = "../src/summary.rs"]
mod summary;

use vstd::prelude::*;

verus! {

/// #823: non-vacuous witnesses exercise each JSON escape-width branch.
fn witnesses() {
    let ascii = summary::escaped_codepoint_bytes(65);
    assert(ascii == 1);
    let control = summary::escaped_codepoint_bytes(10);
    assert(control == 2);
    let zero = summary::escaped_codepoint_bytes(0);
    assert(zero == 6);
    let delete = summary::escaped_codepoint_bytes(127);
    assert(delete == 6);
    let surrogate = summary::escaped_codepoint_bytes(0xd800);
    assert(surrogate == 6);
    let non_bmp = summary::escaped_codepoint_bytes(0x1f600);
    assert(non_bmp == 12);
    let total = summary::size_sum(2, 12);
    assert(total == 14);
    let fits = summary::fits_budget(100, 10, 110);
    assert(fits );
    let exceeds = summary::fits_budget(100, 11, 110);
    assert(exceeds == false);
    let overflow = summary::fits_budget(u64::MAX, 1, u64::MAX);
    assert(overflow == false);
    let full = summary::retention_level(100, 80, 100);
    assert(full == 0);
    let core = summary::retention_level(100, 80, 80);
    assert(core == 1);
    let diagnostic = summary::retention_level(100, 80, 79);
    assert(diagnostic == 2);
    let passing = summary::display_priority(true, false);
    assert(passing == 1);
    let ordinary = summary::display_priority(false, false);
    assert(ordinary == 2);
    let red_pass = summary::display_priority(true, true);
    assert(red_pass == 255);
    let red_error = summary::display_priority(false, true);
    assert(red_error == 255);
    let red = summary::mandatory_entry(true, false, true, false, true, false);
    assert(red);
    let missing_name = summary::mandatory_entry(true, false, false, true, true, false);
    assert(missing_name);
    let symptom = summary::mandatory_entry(false, false, false, true, false, true);
    assert(symptom);
    let display_error = summary::mandatory_entry(false, false, false, true, true, false);
    assert(!display_error);

}

}

fn main() {}
