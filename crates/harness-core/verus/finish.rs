use vstd::prelude::*;
#[path = "../src/finish.rs"]
mod finish;
use finish::{Mode, checking_outcomes, consistent, process_exit, summary_outcomes};

verus! {
/// #799/#804: all i32 return/finish codes and both flags; no numeric or loop bound.
fn exit_mapping(returned: i32, finished: Option<i32>, raised: bool, xdist: bool) {
    let out = process_exit(returned, finished, raised, xdist);
    if raised && (!xdist || finished.is_none()) { assert(out == 3); }
    if !raised { assert(out == returned); }
}

/// #804: internal error is never accepted, including matching finish records in either mode.
fn finish_consistency(mode: Mode, finished: Option<i32>, exit: i32) {
    let accepted = consistent(mode, finished, exit);
    if exit == 3 { assert(!accepted); }
    if accepted {
        assert(finished == Some(exit));
        assert(exit == 0 || exit == 1 || exit == 2 || exit == 4 || exit == 5);
    }
}

/// #804: reachable teardown recovery and ordinary failure witnesses prevent vacuity.
fn finish_witnesses() {
    let teardown = process_exit(0, Some(1), true, true);
    assert(teardown == 1);
    let other = process_exit(0, Some(1), true, false);
    assert(other == 3);
    let valid = consistent(Mode::Checking, Some(1), 1);
    assert(valid);
    let invalid = consistent(Mode::Summary, Some(3), 3);
    assert(!invalid);
    let checking = checking_outcomes(0, true, false);
    assert(checking);
    let summary = summary_outcomes(2, false, false);
    assert(!summary);
}
}
fn main() {}
