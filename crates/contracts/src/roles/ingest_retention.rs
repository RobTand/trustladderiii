//! #840 finite retention decision after source parsing, at the original reply check.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Keep,
    Append,
    Repair,
    Object,
}

/// Ingest/Audit/#840: completeness wins; audit objects; ingest never exceeds maxLength.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == if complete { Action::Keep } else if audit { Action::Object }
        else if fits { Action::Append } else { Action::Repair },
))]
pub fn decide(audit: bool, complete: bool, fits: bool) -> Action {
    if complete {
        Action::Keep
    } else if audit {
        Action::Object
    } else if fits {
        Action::Append
    } else {
        Action::Repair
    }
}
