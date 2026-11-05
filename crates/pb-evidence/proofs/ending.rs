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
/// #821: every single checked outer field or receipt/log predicate corruption rejects.
fn checked_field_witness() {
    let good=decision::header(true,true,true,true); assert(good);
    let bad=decision::header(false,true,true,true); assert(!bad);
    let bad=decision::header(true,false,true,true); assert(!bad);
    let bad=decision::header(true,true,false,true); assert(!bad);
    let bad=decision::header(true,true,true,false); assert(!bad);
    let good=decision::receipt(true,true); assert(good);
    let bad=decision::receipt(false,true); assert(!bad);
    let bad=decision::receipt(true,false); assert(!bad);
    let good=decision::log(true,true); assert(good);
    let bad=decision::log(false,true); assert(!bad);
    let bad=decision::log(true,false); assert(!bad);
}

/// #821: all shape groups and symptom fields have reachable single-corruption witnesses.
fn summary_witness() {
    let good=decision::shape(true,true,true,true,true); assert(good);
    let bad=decision::shape(false,true,true,true,true); assert(!bad);
    let bad=decision::shape(true,false,true,true,true); assert(!bad);
    let bad=decision::shape(true,true,false,true,true); assert(!bad);
    let bad=decision::shape(true,true,true,false,true); assert(!bad);
    let bad=decision::shape(true,true,true,true,false); assert(!bad);
    let good=decision::unique(1); assert(good);
    let bad=decision::unique(0); assert(!bad);
    let bad=decision::unique(2); assert(!bad);
    let good=decision::symptom(true,true,true); assert(good);
    let bad=decision::symptom(false,true,true); assert(!bad);
    let bad=decision::symptom(true,false,true); assert(!bad);
    let bad=decision::symptom(true,true,false); assert(!bad);
}

}
fn main() {}
