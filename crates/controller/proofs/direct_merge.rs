#[path = "../src/adapters/direct_merge/decide.rs"]
mod decide;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(verus_keep_ghost)]
verus! {
/// #830's approved direct-delivery rule; its "Q44 approval check" wording refers
/// to the retained publishing step at docs/state-table.md:531. Exact-head
/// approval is Q41 (docs/state-machine.md:132,487), not canonical Q44 (:495).
/// Publishing/Queued docs/state-table.md:526,570; outside push :667; epoch :755.
/// Witnesses reach both upstream writes, refusal, and the absorbing merged state.
/// No loop bound: the production decision is straight-line over arbitrary full SHA values.
fn witnesses() {
    let h = decide::Head { upper: 1, lower: 2 };
    let visible = decide::Facts { approved: h, write_authorized: true, observed: h, checked: h, checking: true, approval: true,
        open: true, merged: false, recorded_merge: false, held: false, conflict: false,
        checks: decide::Checks::Passed, visible: true };
    let merge = decide::decide(visible);
    assert(merge.third == decide::Step::Merge);
    assert(merge.first == decide::Step::Fence && merge.second == decide::Step::Intent);
    let timer = decide::decide(decide::Facts { write_authorized: false, ..visible });
    assert(timer.third == decide::Step::None);
    let unapproved = decide::decide(decide::Facts { approval: false, ..visible });
    assert(unapproved.third == decide::Step::None);
    let unchecked = decide::decide(decide::Facts { checking: false, ..visible });
    assert(unchecked.third == decide::Step::None);
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
