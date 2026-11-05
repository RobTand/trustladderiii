//! #823 review: verify the executable reduction reference used by proptest.
//! All finite candidate slices and u64 sizes; no bounded-unroll assumption.
#[path = "../tests/summary_bound/reference.rs"]
mod reference;
use vstd::prelude::*;
verus! {
/// #823(a-d): witnesses reach full, ordered-prefix, core, diagnostic and error.
fn witnesses() {
    let full = reference::reduce(100, 50, 20, &[120, 90, 50], 100);
    assert(full.kind == 0 && full.bytes == 100);
    let prefix = reference::reduce(100, 50, 20, &[120, 90, 50], 90);
    assert(prefix.kind == 1 && prefix.removed == 1 && prefix.bytes == 90);
    let core = reference::reduce(100, 50, 20, &[120, 90, 60], 50);
    assert(core.kind == 2 && core.bytes == 50);
    let diagnostic = reference::reduce(100, 50, 20, &[120, 90, 60], 20);
    assert(diagnostic.kind == 3 && diagnostic.bytes == 20);
    let error = reference::reduce(100, 50, 20, &[], 19);
    assert(error.kind == 4);
    let mandatory = reference::keeps(prefix, 0, true);
    assert(mandatory);
    let display = reference::keeps(prefix, 0, false);
    assert(!display);
    let priority = reference::precedes(1, 5, 2, 0);
    assert(priority);
    let tie = reference::precedes(2, 0, 2, 1);
    assert(tie);
    let transitive = reference::priority_transitive(1, 4, 2, 3, 4, 2);
    assert(transitive);
    let prefix_order = reference::prefix_preserves_order(prefix, 0, 2);
    assert(prefix_order);
}
}
fn main() {}
