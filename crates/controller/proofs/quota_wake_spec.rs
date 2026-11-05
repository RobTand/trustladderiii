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
    assert(clears_hold(true, true, 11, 10));
    assert(!clears_hold(true, true, 10, 10));
    assert(!clears_hold(true, true, 9, 10));
    assert(!clears_hold(false, true, 11, 10));
    assert(!clears_hold(true, false, 11, 10));
}
}
