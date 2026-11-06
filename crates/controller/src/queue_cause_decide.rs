//! #854b, program lead 2026-11-06 under Q50/Q51. Scalar, production decisions.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attribution {
    Change,
    Environment,
    Unattributed,
}

/// Environment requires an independent, same-base fingerprint match. After the
/// shared retry has been spent, every subsequent failure uses today's Q54 path.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    ensures
        out == (if retried { Attribution::Change } else if matching { Attribution::Environment } else if positive_change { Attribution::Change } else { Attribution::Unattributed }),
        (out == Attribution::Environment) ==> (matching && !retried),
        retried ==> (out == Attribution::Change),
        (!retried && !matching && !positive_change) ==> (out == Attribution::Unattributed),
))]
pub fn attribute(positive_change: bool, matching: bool, retried: bool) -> Attribution {
    if retried {
        Attribution::Change
    } else if matching {
        Attribution::Environment
    } else if positive_change {
        Attribution::Change
    } else {
        Attribution::Unattributed
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldAction {
    Wait,
    Requeue,
    Incident,
}

/// The persisted command budget is consumed before sending. Q58 and #854 share
/// it. At the existing allowance bound, a wait ends TL3-side, never via Rob.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    ensures
        out == (if witness && !spent { HoldAction::Requeue } else if expired { HoldAction::Incident } else { HoldAction::Wait }),
        (out == HoldAction::Requeue) ==> (witness && !spent),
        spent ==> (out != HoldAction::Requeue),
        expired ==> (out != HoldAction::Wait),
        (!witness && expired) ==> (out == HoldAction::Incident),
))]
pub fn hold(witness: bool, spent: bool, expired: bool) -> HoldAction {
    if witness && !spent {
        HoldAction::Requeue
    } else if expired {
        HoldAction::Incident
    } else {
        HoldAction::Wait
    }
}

/// #854b / Q50: Unattributed and Environment never enter a charged failed round
/// or Q54. Their only actions are the bounded hold, retest and TL3-side incident.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    ensures out == (cause == Attribution::Change),
))]
pub fn failed_round(cause: Attribution) -> bool {
    cause == Attribution::Change
}
