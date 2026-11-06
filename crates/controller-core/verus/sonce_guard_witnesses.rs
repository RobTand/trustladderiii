use crate::translate::*;
use vstd::prelude::*;

verus! {
/// S-once / #751/#760: a recorded receipt reaches the actual early return.
/// lib.rs:2846-2850 on f5b0ba2f; no sequence bound is imposed on the guard.
pub fn recorded_witness() {
    let translation = begin(true);
    assert(translation.is_none());
}

/// S-once / #751/#760: a first receipt reaches the actual kernel dispatch.
/// Q52's non-kernel cases remain covered by the existing dispatch witnesses.
pub fn first_kernel_witness() {
    let translation = begin(false);
    assert(translation.is_some());
    let decision = dispatch(translation.unwrap(), Route::Kernel(KernelKind::Conflict), EvidenceKind::Other);
    assert(decision == Decision::Kernel(KernelKind::Conflict));
}
}
