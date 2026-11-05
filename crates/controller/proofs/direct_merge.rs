#[path = "../src/adapters/direct_merge/decide.rs"]
mod decide;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(verus_keep_ghost)]
verus! {
/// #830/Q44, Publishing/Queued docs/state-table.md:526,570; outside push :667.
/// Witnesses reach both upstream writes, refusal, and the absorbing merged state.
/// No loop bound: the production decision is straight-line over arbitrary full SHA values.
proof fn witnesses() {
    let h = decide::Head { upper: 1, lower: 2 };
    let visible = decide::Facts { approved: h, observed: h, checked: h, approval: true,
        open: true, merged: false, recorded_merge: false, held: false, conflict: false,
        checks: decide::Checks::Passed, visible: true };
    let merge = decide::decide(visible);
    assert(merge.third == decide::Step::Merge);
    assert(merge.first == decide::Step::Fence && merge.second == decide::Step::Intent);
    let annotation = decide::decide(decide::Facts { visible: false, ..visible });
    assert(annotation.third == decide::Step::Annotate);
    let moved = decide::decide(decide::Facts { observed: decide::Head { upper: 3, lower: 2 }, ..visible });
    assert(moved.outcome == decide::Outcome::Moved);
    let recorded = decide::decide(decide::Facts { recorded_merge: true, ..visible });
    assert(recorded.third == decide::Step::None);
    let observed = decide::decide(decide::Facts { merged: true, ..visible });
    assert(observed.third == decide::Step::None);
}
}
fn main() {}
