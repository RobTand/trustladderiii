//! Q53 / #828: only later success on the same model and family clears a hold.

#[cfg(verus_keep_ghost)]
use vstd::prelude::SpecOrd;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(clears => ensures
    clears == (same_model && same_family && success_ms > quota_ms),
))]
pub fn clears_hold(same_model: bool, same_family: bool, success_ms: i64, quota_ms: i64) -> bool {
    same_model && same_family && success_ms > quota_ms
}
