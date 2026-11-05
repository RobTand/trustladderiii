//! #706: summary and checking use distinct folds over pytest phase facts.

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Setup,
    Call,
    Teardown,
    WorkerCrash,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Passed,
    Failed,
    Skipped,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fact {
    pub phase: Phase,
    pub status: Status,
    pub assertion: bool,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fold {
    Passed,
    Failed,
    Error,
    Skipped,
}

/// #706: failed phases dominate skipped phases in summary mode.
/// One fold step; induction applies to any number of phase facts, including xdist crashes.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    matches!(out, Fold::Error) == (matches!(folded, Fold::Error)
        || matches!(fact.status, Status::Failed)
            && (!matches!(fact.phase, Phase::Call) || !fact.assertion)),
    matches!(out, Fold::Failed) == (!matches!(out, Fold::Error)
        && (matches!(folded, Fold::Failed)
            || matches!(fact.status, Status::Failed)
                && matches!(fact.phase, Phase::Call) && fact.assertion)),
    matches!(out, Fold::Skipped) == (!matches!(out, Fold::Error | Fold::Failed)
        && (matches!(folded, Fold::Skipped) || matches!(fact.status, Status::Skipped))),
    matches!(out, Fold::Passed) == (matches!(folded, Fold::Passed)
        && matches!(fact.status, Status::Passed)),
))]
pub fn summary_step(folded: Fold, fact: Fact) -> Fold {
    if matches!(folded, Fold::Error)
        || matches!(fact.status, Status::Failed)
            && (!matches!(fact.phase, Phase::Call) || !fact.assertion)
    {
        Fold::Error
    } else if matches!(folded, Fold::Failed)
        || matches!(fact.status, Status::Failed)
            && matches!(fact.phase, Phase::Call)
            && fact.assertion
    {
        Fold::Failed
    } else if matches!(folded, Fold::Skipped) || matches!(fact.status, Status::Skipped) {
        Fold::Skipped
    } else {
        Fold::Passed
    }
}

/// #706: check mode skips before interpreting a call failure; external failures dominate.
/// Missing call without a skip is incomplete, represented by None.
/// #706: three phase slots, optional call; no population-fold or AddedName claim.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (if matches!(setup, Status::Failed) || matches!(teardown, Status::Failed) {
        Some(Fold::Error)
    } else if matches!(setup, Status::Skipped) || matches!(teardown, Status::Skipped)
        || matches!(call, Some(Fact { status: Status::Skipped, .. })) {
        Some(Fold::Skipped)
    } else { match call {
        None => None,
        Some(fact) => Some(if matches!(fact.status, Status::Failed) {
            if fact.assertion { Fold::Failed } else { Fold::Error }
        } else { Fold::Passed }),
    }}),
))]
pub fn checking(setup: Status, call: Option<Fact>, teardown: Status) -> Option<Fold> {
    if matches!(setup, Status::Failed) || matches!(teardown, Status::Failed) {
        return Some(Fold::Error);
    }
    if matches!(setup, Status::Skipped)
        || matches!(teardown, Status::Skipped)
        || matches!(
            call,
            Some(Fact {
                status: Status::Skipped,
                ..
            })
        )
    {
        return Some(Fold::Skipped);
    }
    let fact = match call {
        Some(fact) => fact,
        None => return None,
    };
    Some(if matches!(fact.status, Status::Failed) {
        if fact.assertion {
            Fold::Failed
        } else {
            Fold::Error
        }
    } else {
        Fold::Passed
    })
}
