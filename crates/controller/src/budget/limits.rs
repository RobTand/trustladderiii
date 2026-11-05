#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
/// Q4: the fit limit is exclusive.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == (size < limit),
))]
pub fn fits(size: u64, limit: u64) -> bool {
    size < limit
}

/// #698/#718: counters never wrap and remaining allowance cannot be negative.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result <= allowed, result as int == if used <= allowed { allowed as int - used as int } else { 0 },
))]
pub fn remaining(used: u64, allowed: u64) -> u64 {
    allowed.saturating_sub(used)
}

/// #698: ineffective compaction must be reported, never counted as freed room.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == (before > after),
))]
pub fn compacted(before: u64, after: u64) -> bool {
    after < before
}

/// #698/#718: finalization reserves are verification-only and survive recovery.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == (maker && (turns as int + (if reserve < allowed { reserve } else { allowed }) as int >= allowed as int
        || (window != 0 && no_progress as int + (if reserve < allowed { reserve } else { allowed }) as int >= window as int))),
))]
pub fn finalizing(
    maker: bool,
    turns: u64,
    no_progress: u64,
    allowed: u64,
    window: u64,
    reserve: u64,
) -> bool {
    maker
        && (turns.saturating_add(reserve.min(allowed)) >= allowed
            || (window != 0 && no_progress.saturating_add(reserve.min(allowed)) >= window))
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exhaustion {
    Backstop,
    ToolTurns,
    NoProgress,
    ContextTokens,
    InputTokens,
}

/// State-table execution feasibility (#614/#692): ordered, distinct limits.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug)]
pub struct Run {
    pub maker: bool,
    pub turns: u64,
    pub allowed: u64,
    pub stalled: bool,
    pub context_full: bool,
    pub spent: u64,
    pub predicted: u64,
    pub reserve: u64,
    pub input_limit: u64,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == expected_exhaustion(run),
))]
pub fn exhaustion(run: Run) -> Option<Exhaustion> {
    let Run {
        maker,
        turns,
        allowed,
        stalled,
        context_full,
        spent,
        predicted,
        reserve,
        input_limit,
    } = run;
    if turns >= allowed {
        Some(if maker {
            Exhaustion::Backstop
        } else {
            Exhaustion::ToolTurns
        })
    } else if stalled {
        Some(Exhaustion::NoProgress)
    } else if context_full {
        Some(Exhaustion::ContextTokens)
    } else if u128::from(spent) + u128::from(predicted) + u128::from(reserve)
        > u128::from(input_limit)
    {
        Some(Exhaustion::InputTokens)
    } else {
        None
    }
}

#[cfg(verus_keep_ghost)]
verus! {
/// #698/#718: independent wide-arithmetic oracle and ordered stop reasons.
pub open spec fn expected_exhaustion(r: Run) -> Option<Exhaustion> {
    if r.turns >= r.allowed { Some(if r.maker { Exhaustion::Backstop } else { Exhaustion::ToolTurns }) }
    else if r.stalled { Some(Exhaustion::NoProgress) }
    else if r.context_full { Some(Exhaustion::ContextTokens) }
    else if r.spent as int + r.predicted as int + r.reserve as int > r.input_limit as int { Some(Exhaustion::InputTokens) }
    else { None }
}
}
