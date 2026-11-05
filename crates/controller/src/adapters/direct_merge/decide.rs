#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// A SHA-1 split without hashing or truncation. The I/O boundary parses 40 hex digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub struct Head {
    pub upper: u128,
    pub lower: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub enum Checks {
    Passed,
    Pending,
    Failed,
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct Facts {
    pub approved: Head,
    pub write_authorized: bool,
    pub observed: Head,
    pub checked: Head,
    pub checking: bool,
    pub approval: bool,
    pub open: bool,
    pub merged: bool,
    pub recorded_merge: bool,
    pub held: bool,
    pub conflict: bool,
    pub checks: Checks,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub enum Step {
    None,
    Fence,
    Intent,
    Annotate,
    Merge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub enum Outcome {
    Ready,
    Observe,
    Merged,
    Moved,
    Failed,
    Conflict,
    Wait,
    Held,
    Closed,
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct Plan {
    pub outcome: Outcome,
    pub first: Step,
    pub second: Step,
    pub third: Step,
    pub sha: Head,
}

/// #830 is the approved source of direct delivery and its retained approval check
/// (called "Q44" in #830; canonical Q44 is reviewer context exhaustion).
/// Exact-head approval is Q41, docs/state-machine.md:132,487; passed Checking and
/// approval: docs/state-table.md:507,518; approval-check publishing step: :531.
/// Base Publishing/Queued rows :526,:570; outside push :667; effects :724-747;
/// epoch fence :755. No-write after a known merge refines #830's Converges rule.
/// #830 retry clarification: writes require restart/event authorization; timers only observe.
/// Straight-line, unbounded in head values. Each write plan is executed in field order.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.sha.upper == f.approved.upper && out.sha.lower == f.approved.lower,
    (out.third == Step::Merge || out.third == Step::Annotate) ==>
        out.first == Step::Fence && out.second == Step::Intent,
    out.third == Step::Merge ==> !f.merged && !f.recorded_merge && f.open
        && f.write_authorized && !f.held && !f.conflict && f.visible && f.approval && f.checking
        && f.checks == Checks::Passed
        && f.observed.upper == f.approved.upper && f.observed.lower == f.approved.lower
        && f.checked.upper == f.approved.upper && f.checked.lower == f.approved.lower,
    !f.merged && !f.recorded_merge &&
        (f.observed.upper != f.approved.upper || f.observed.lower != f.approved.lower)
        ==> out.outcome == Outcome::Moved && out.third == Step::None,
    (!f.checking || !f.approval || !f.write_authorized) ==> out.third == Step::None,
    (f.merged || f.recorded_merge) ==> out.outcome == Outcome::Merged && out.third == Step::None,
    !f.merged && !f.recorded_merge && f.open && f.write_authorized && !f.held && !f.conflict && f.approval && f.checking
        && f.checks == Checks::Passed
        && f.observed.upper == f.approved.upper && f.observed.lower == f.approved.lower
        && f.checked.upper == f.approved.upper && f.checked.lower == f.approved.lower
        ==> out.outcome == Outcome::Ready && out.third == (if f.visible { Step::Merge } else { Step::Annotate }),
))]
pub fn decide(f: Facts) -> Plan {
    let outcome = if f.merged || f.recorded_merge {
        Outcome::Merged
    } else if f.observed.upper != f.approved.upper || f.observed.lower != f.approved.lower {
        Outcome::Moved
    } else if !f.open {
        Outcome::Closed
    } else if f.held {
        Outcome::Held
    } else if f.conflict {
        Outcome::Conflict
    } else if matches!(f.checks, Checks::Failed) {
        Outcome::Failed
    } else if !f.checking
        || !f.approval
        || !matches!(f.checks, Checks::Passed)
        || f.checked.upper != f.approved.upper
        || f.checked.lower != f.approved.lower
    {
        Outcome::Wait
    } else if !f.write_authorized {
        Outcome::Observe
    } else {
        Outcome::Ready
    };
    let ready = matches!(outcome, Outcome::Ready);
    Plan {
        outcome,
        first: if ready { Step::Fence } else { Step::None },
        second: if ready { Step::Intent } else { Step::None },
        third: if !ready {
            Step::None
        } else if f.visible {
            Step::Merge
        } else {
            Step::Annotate
        },
        sha: f.approved,
    }
}
