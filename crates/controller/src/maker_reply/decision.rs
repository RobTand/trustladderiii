//! Finite facts derived from the typed goal and reproduction collections.

/// Q54 maker reply: equal lengths and equal identity sets give exact coverage.
/// All u64 lengths and both identity-set outcomes; no collection-size bound.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (expected == actual && same_ids),
))]
pub fn exact_coverage(expected: u64, actual: u64, same_ids: bool) -> bool {
    expected == actual && same_ids
}

/// Q54 maker reply: only maker goals may be met before the owning downstream stage.
/// Exhaustive Boolean domain, without a goal-count bound.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (if maker { met } else { pending }),
))]
pub fn stage_valid(maker: bool, met: bool, pending: bool) -> bool {
    if maker { met } else { pending }
}
