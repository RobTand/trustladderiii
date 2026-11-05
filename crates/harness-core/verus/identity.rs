//! #704/#723/#823: equality of a typed identity requires every field.
//! All sixteen Boolean inputs are covered by the unbounded decision theorem.
//! SHA-256 collision resistance is outside this theorem.
#[path = "../src/identity.rs"]
mod identity;
use vstd::prelude::*;
verus! {
/// #704: witnesses exercise acceptance and each independently rejected field.
fn witnesses() {
    let case_0 = identity::identity_fields_match(true, true, true, true);
    assert(case_0 == true);
    let case_1 = identity::identity_fields_match(false, true, true, true);
    assert(case_1 == false);
    let case_2 = identity::identity_fields_match(true, false, true, true);
    assert(case_2 == false);
    let case_3 = identity::identity_fields_match(true, true, false, true);
    assert(case_3 == false);
    let case_4 = identity::identity_fields_match(true, true, true, false);
    assert(case_4 == false);

}
}
fn main() {}
