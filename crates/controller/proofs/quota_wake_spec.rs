use crate::quota_wake::clears_hold;
use vstd::prelude::*;

verus! {
/// Q53 / #828: exact identity and strictly later evidence are necessary and sufficient.
/// All Boolean identities and all i64 timestamps, without a sequence or unwind bound.
fn ordered_identity(same_model: bool, same_family: bool, success_ms: i64, quota_ms: i64) {
    let clears = clears_hold(same_model, same_family, success_ms, quota_ms);
    assert(clears <==> (same_model && same_family && success_ms > quota_ms));
}

/// Q53 / #828: explicit reachable clear and reject witnesses prevent vacuity.
fn witnesses() {
    let later = clears_hold(true, true, 11, 10);
    let equal = clears_hold(true, true, 10, 10);
    let earlier = clears_hold(true, true, 9, 10);
    let other_model = clears_hold(false, true, 11, 10);
    let other_family = clears_hold(true, false, 11, 10);
    assert(later);
    assert(!equal);
    assert(!earlier);
    assert(!other_model);
    assert(!other_family);
}
}
