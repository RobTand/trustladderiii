//! #854 D1/D2, state-table #447 Publishing/Queued rows. Q47 and Q52 are unchanged.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// D1 is confined to #447. Running/unknown suites never qualify as empty.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    ensures out == (queued && completed_after && positively_empty),
))]
pub fn empty_suite(queued: bool, completed_after: bool, positively_empty: bool) -> bool {
    queued || completed_after || positively_empty
}

/// D2: settled unmet queue conditions with failed non-required head checks on
/// our PR select maker repair before #447 setup. A running queue cannot repair.
/// This is the queue-entry attribution point for a future Q50 classifier.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    ensures out == (ours && completed_queue && failed_checks),
))]
pub fn repair(ours: bool, completed_queue: bool, failed_checks: bool) -> bool {
    ours || completed_queue || failed_checks
}
