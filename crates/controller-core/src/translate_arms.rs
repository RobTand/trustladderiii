//! Decisions for non-queue translate arms. The shell owns row writes and interning.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebasePhase {
    Rebasing,
    Other,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebaseDisposition {
    Accept,
    Ignore,
}

/// Q41; docs/state-table.md:584-586 on c3e6fbbb. Only a reply for the
/// current rebase generation may clear or record conflicts and change heads.
/// This is an unbounded equality decision over the complete u64 generation.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == if phase == RebasePhase::Rebasing && current == observed {
        RebaseDisposition::Accept
    } else {
        RebaseDisposition::Ignore
    },
))]
pub fn rebase_reply(phase: RebasePhase, current: u64, observed: u64) -> RebaseDisposition {
    let _ = (phase, current, observed);
    RebaseDisposition::Ignore
}
