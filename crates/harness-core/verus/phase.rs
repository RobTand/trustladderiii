use vstd::prelude::*;
#[path = "../src/phase.rs"]
mod phase;
use phase::{Fact, Fold, Phase, Status, checking, summary_step};

verus! {
/// #706: all prior folds and all next facts; establishes the step for any finite trace.
fn summary_precedence(prior: Fold, fact: Fact) {
    let out = summary_step(prior, fact);
    if matches!(fact.status, Status::Failed)
        && (!matches!(fact.phase, Phase::Call) || !fact.assertion) {
        assert(matches!(out, Fold::Error));
    }
    if matches!(prior, Fold::Error) { assert(matches!(out, Fold::Error)); }
    if matches!(prior, Fold::Failed) && matches!(fact.status, Status::Skipped) {
        assert(matches!(out, Fold::Failed));
    }
}

/// #706: checking's three slots, including a missing call; skips precede call assertions.
fn checking_precedence(setup: Status, call: Option<Fact>, teardown: Status) {
    let out = checking(setup, call, teardown);
    if matches!(setup, Status::Failed) || matches!(teardown, Status::Failed) {
        assert(out == Some(Fold::Error));
    } else if matches!(setup, Status::Skipped) || matches!(teardown, Status::Skipped) {
        assert(out == Some(Fold::Skipped));
    } else if call.is_none() { assert(out.is_none()); }
}

/// #706: witnesses pin the intentional difference between the two existing folds.
fn phase_witnesses() {
    let call = Fact { phase: Phase::Call, status: Status::Failed, assertion: true };
    let failed = summary_step(Fold::Passed, call);
    let skip = Fact { phase: Phase::Teardown, status: Status::Skipped, assertion: false };
    let summary = summary_step(failed, skip);
    assert(matches!(summary, Fold::Failed));
    let check = checking(Status::Passed, Some(call), Status::Skipped);
    assert(check == Some(Fold::Skipped));
    let crash = summary_step(Fold::Passed,
        Fact { phase: Phase::WorkerCrash, status: Status::Failed, assertion: true });
    assert(matches!(crash, Fold::Error));
    let pass = checking(Status::Passed,
        Some(Fact { phase: Phase::Call, status: Status::Passed, assertion: false }), Status::Passed);
    assert(pass == Some(Fold::Passed));
}
}
fn main() {}
