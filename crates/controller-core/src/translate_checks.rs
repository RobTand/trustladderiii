//! Preparation and check-reply decisions. Reads, JSON and row writes stay in translate.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preparation {
    Ignore,
    SavePlan,
}

/// Q45, docs/state-table.md:483,496-500 on 751c00ac. An adopted head is pinned.
/// Saving its check plan only on that head is preserved existing behaviour (no rule).
/// Both equality outcomes and the full adopted-bit domain are covered.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == if adopted && same_head { Preparation::SavePlan } else { Preparation::Ignore },
))]
pub fn preparation(adopted: bool, same_head: bool) -> Preparation {
    Preparation::Ignore
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanUpdate {
    KeepFixes,
    Retarget { set_reviewed: bool, replace_tests: bool },
}

/// Q42, docs/state-machine.md:109 and docs/state-table.md:234-244 on 751c00ac.
/// A finding without a reproducing test needs a reason. Retargeting only matching
/// stored heads, and retaining reviewed_sha when absent, preserve existing behaviour
/// (no rule). The shell preserves optional-head equality, including None == None.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == if same_head { PlanUpdate::Retarget { set_reviewed: reviewed, replace_tests: unavailable } }
        else { PlanUpdate::KeepFixes },
))]
pub fn plan_update(same_head: bool, reviewed: bool, unavailable: bool) -> PlanUpdate {
    PlanUpdate::KeepFixes
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChecksDecision {
    pub record: bool,
    pub pass: bool,
}

/// Q41, docs/state-machine.md:132; docs/state-table.md:509-510,584 on 751c00ac.
/// Checks on a stale head cannot pass. The generation/job evidence-write guard is
/// preserved existing behaviour (no rule). Equalities cover all u64 generations.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result.record == (job == Some(observed) && current == observed && exact_head),
    result.pass == (passed && exact_head),
))]
pub fn checks_reply(job: Option<u64>, current: u64, observed: u64, exact_head: bool, passed: bool) -> ChecksDecision {
    ChecksDecision { record: false, pass: false }
}
