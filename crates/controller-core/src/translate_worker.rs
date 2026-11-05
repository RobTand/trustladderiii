//! Decisions in the WorkerReply and WorkerFailed translation arms.
//! Wire parsing, delegate calls and every transaction operation stay in the shell.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(not(verus_keep_ghost))]
macro_rules! scan {
    (($condition:expr) [$($contract:tt)*] $body:block) => { while $condition $body };
}
#[cfg(verus_keep_ghost)]
macro_rules! scan {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! { while $condition $($contract)* $body }
    };
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Ingesting,
    Auditing,
    Planning,
    Making,
    Reviewing,
    Other,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Ingest,
    Audit,
    Planner,
    Maker,
    Reviewer,
    Writer,
    Classifier,
    Postmortem,
    Unknown,
}

/// Base 6ba8d25035, docs/state-table.md:139 requires matching operation kind.
/// The exact five admitted pairs are preserved existing behaviour (no rule).
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == ((phase == Phase::Ingesting && role == Role::Ingest)
        || (phase == Phase::Auditing && role == Role::Audit)
        || (phase == Phase::Planning && role == Role::Planner)
        || (phase == Phase::Making && role == Role::Maker)
        || (phase == Phase::Reviewing && role == Role::Reviewer)),
))]
pub fn valid_role(phase: Phase, role: Role) -> bool {
    matches!(
        (phase, role),
        (Phase::Ingesting, Role::Ingest)
            | (Phase::Auditing, Role::Audit)
            | (Phase::Planning, Role::Planner)
            | (Phase::Making, Role::Maker)
            | (Phase::Reviewing, Role::Reviewer)
    )
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Ignore,
    RunFailed,
}

/// Base 6ba8d25035, docs/state-table.md:139 defines current-operation staleness.
/// Ignoring stale replies and routing all accepted WorkerFailed classes to
/// RunFailed are preserved existing behaviour (no rule). No credit decision
/// or retry decision is made here.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if run == Some(generation) && current == generation {
        Failure::RunFailed
    } else { Failure::Ignore },
))]
pub fn worker_failed(run: Option<u64>, current: u64, generation: u64) -> Failure {
    if run != Some(generation) || current != generation {
        Failure::Ignore
    } else {
        Failure::RunFailed
    }
}

/// Base 6ba8d25035, docs/state-table.md:485: maker done checks every measure.
/// The set equality, count equality and accepted-stage handover conjunction is
/// preserved existing behaviour (no rule). Parsing and handover stay delegated.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (same_goals && same_count && handover_valid),
))]
pub fn maker_self_check(same_goals: bool, same_count: bool, handover_valid: bool) -> bool {
    same_goals && same_count && handover_valid
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Document {
    Briefs,
    Audits,
    MakerReplies,
    Plans,
}

/// Preserved existing behaviour (no rule), base lib.rs:3434-3440.
/// The shell's role-specific kernel mapper remains authoritative. Reviewer
/// replies write evidence/session rows and have no general document row.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match role {
        Role::Ingest => Some(Document::Briefs),
        Role::Audit => Some(Document::Audits),
        Role::Maker => Some(Document::MakerReplies),
        Role::Reviewer => None,
        _ => Some(Document::Plans),
    },
))]
pub fn document(role: Role) -> Option<Document> {
    match role {
        Role::Ingest => Some(Document::Briefs),
        Role::Audit => Some(Document::Audits),
        Role::Maker => Some(Document::MakerReplies),
        Role::Reviewer => None,
        _ => Some(Document::Plans),
    }
}

#[cfg(verus_keep_ghost)]
vstd::prelude::verus! {
    /// Mathematical prefix cost; unbounded sequence, no machine overflow.
    pub open spec fn spent(lengths: Seq<usize>, count: int) -> int
        decreases count,
    {
        if count <= 0 { 0 } else { spent(lengths, count - 1) + lengths[count - 1] as int }
    }
}

/// Preserved existing behaviour (no rule), base lib.rs:3387-3403.
/// Select the longest initial prefix allowed by count and Unicode-character
/// budget. A later short report cannot jump past the first report that fails.
/// The proof covers arbitrary slice lengths; there is no bounded unwind.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out <= lengths.len(), out <= limit,
    0 <= spent(lengths@, out as int) <= budget,
    out < lengths.len() && out < limit ==>
        spent(lengths@, out as int) + lengths[out as int] > budget,
))]
pub fn findings_prefix(lengths: &[usize], limit: usize, budget: usize) -> usize {
    let mut count = 0;
    let mut remaining = budget;
    scan! { (count < lengths.len() && count < limit) [
        invariant count <= lengths.len(), count <= limit, remaining <= budget,
            spent(lengths@, count as int) == budget - remaining,
        decreases lengths.len() - count,
    ] {
        let length = lengths[count];
        if length > remaining { return count; }
        remaining -= length;
        count += 1;
    } }
    count
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewVerdict {
    Approve,
    Changes,
    Other,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewRoute {
    Approve(u64),
    MissingHead,
    Changes,
    Keep,
}

/// Base 6ba8d25035, docs/state-table.md:518-519: approval carries the checked
/// head; changes request a failed round. Exact missing-head error and Other
/// pass-through are preserved existing behaviour (no rule).
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match verdict {
        ReviewVerdict::Approve => match head {
            Some(head) => ReviewRoute::Approve(head), None => ReviewRoute::MissingHead,
        },
        ReviewVerdict::Changes => ReviewRoute::Changes,
        ReviewVerdict::Other => ReviewRoute::Keep,
    },
))]
pub fn review_route(verdict: ReviewVerdict, head: Option<u64>) -> ReviewRoute {
    match verdict {
        ReviewVerdict::Approve => match head {
            Some(head) => ReviewRoute::Approve(head),
            None => ReviewRoute::MissingHead,
        },
        ReviewVerdict::Changes => ReviewRoute::Changes,
        ReviewVerdict::Other => ReviewRoute::Keep,
    }
}
