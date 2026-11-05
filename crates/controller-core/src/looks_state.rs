//! Look results, declared waits, and model quota decisions. The shell owns I/O.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(verus_keep_ghost)]
verus! {
    pub open spec fn capped_add(a: u64, b: u64) -> u64 {
        if a as int + b as int > u64::MAX as int { u64::MAX } else { (a + b) as u64 }
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaWait {
    Resume,
    Wait { due: u64 },
    Notify { due: u64 },
}

/// Q53, state-table.md:277,792 / #759: wake at the earlier reset or one ladder-out bound.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    (out == QuotaWait::Resume) == (resume <= now),
    match out {
        QuotaWait::Resume => true,
        QuotaWait::Wait { due } => now < due && due <= resume,
        QuotaWait::Notify { due } => due == resume && !noticed && since.is_some(),
    },
    out == if resume <= now { QuotaWait::Resume } else {
        match since {
            Some(start) => if !noticed {
                let bound = capped_add(start, delay);
                if bound <= now { QuotaWait::Notify { due: resume } }
                else { QuotaWait::Wait { due: if bound < resume { bound } else { resume } } }
            } else { QuotaWait::Wait { due: resume } },
            None => QuotaWait::Wait { due: resume },
        }
    },
))]
pub fn quota_wait(
    resume: u64,
    now: u64,
    since: Option<u64>,
    noticed: bool,
    delay: u64,
) -> QuotaWait {
    if resume <= now {
        return QuotaWait::Resume;
    }
    match since {
        Some(start) => {
            if !noticed {
                let bound = start.saturating_add(delay);
                if bound <= now {
                    QuotaWait::Notify { due: resume }
                } else {
                    QuotaWait::Wait {
                        due: if bound < resume { bound } else { resume },
                    }
                }
            } else {
                QuotaWait::Wait { due: resume }
            }
        }
        None => QuotaWait::Wait { due: resume },
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryAction {
    ResumeQuota,
    FinishStall,
    Look,
}
/// Preserved existing behaviour (no rule): quota recovery precedes stopping and the upstream read.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if quota { EntryAction::ResumeQuota } else if stopping { EntryAction::FinishStall } else { EntryAction::Look },
))]
pub fn entry_action(quota: bool, stopping: bool) -> EntryAction {
    if quota {
        EntryAction::ResumeQuota
    } else if stopping {
        EntryAction::FinishStall
    } else {
        EntryAction::Look
    }
}

/// Preserved existing behaviour (no rule): only an effect's repeated GitHub not-found becomes authoritative absence.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (github_404 && previous_not_found && effect),))]
pub fn missing_is_absent(github_404: bool, previous_not_found: bool, effect: bool) -> bool {
    github_404 && previous_not_found && effect
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookKind {
    Found,
    Absent,
    Alive,
    Waiting,
    Healthy,
    Quota,
    Pending,
    Failed,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadDecision {
    pub reset_failures: bool,
    pub clear_wait: bool,
}
/// Preserved existing behaviour (no rule): waiting/pending/alive keep counters for their own handling.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.reset_failures == (kind != LookKind::Waiting && kind != LookKind::Pending && kind != LookKind::Alive && kind != LookKind::Failed),
    out.clear_wait == (has_wait && kind != LookKind::Waiting),
))]
pub fn read_decision(kind: LookKind, has_wait: bool) -> ReadDecision {
    ReadDecision {
        reset_failures: !matches!(
            kind,
            LookKind::Waiting | LookKind::Pending | LookKind::Alive | LookKind::Failed
        ),
        clear_wait: has_wait && kind != LookKind::Waiting,
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoundAction {
    Skip,
    Complete,
    Observe,
}
/// Preserved existing behaviour (no rule): pending completions acknowledge; settled effects observe setup gaps or changed nonempty facts.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    (out == FoundAction::Complete) == pending,
    (out == FoundAction::Observe) == (!pending && (setup_gap || (changed && observations))),
    (out == FoundAction::Skip) == (!pending && !setup_gap && !(changed && observations)),
))]
pub fn found_action(
    pending: bool,
    setup_gap: bool,
    changed: bool,
    observations: bool,
) -> FoundAction {
    if pending {
        FoundAction::Complete
    } else if setup_gap || (changed && observations) {
        FoundAction::Observe
    } else {
        FoundAction::Skip
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbsenceAction {
    ReleaseHold,
    Retry,
    Ignore,
}
/// State-table.md:551 at cdacce8f: waiting for required checks spends no retry. Other absence gates are preserved existing behaviour (no rule).
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    (out == AbsenceAction::ReleaseHold) == checks_pending,
    (out == AbsenceAction::Retry) == (!checks_pending && (held || not_found) && pending && !cancelling && effect),
    (out == AbsenceAction::Ignore) == (!checks_pending && !((held || not_found) && pending && !cancelling && effect)),
))]
pub fn absence_action(
    checks_pending: bool,
    held: bool,
    not_found: bool,
    pending: bool,
    cancelling: bool,
    effect: bool,
) -> AbsenceAction {
    if checks_pending {
        AbsenceAction::ReleaseHold
    } else if (held || not_found) && pending && !cancelling && effect {
        AbsenceAction::Retry
    } else {
        AbsenceAction::Ignore
    }
}

/// #373: changed or timely alive looks reset the failure streak. Short-circuit effect-read placement is preserved existing behaviour (no rule).
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (changed || if now >= progress { now - progress < allowance } else { allowance > 0 }),
))]
pub fn alive_resets_early(changed: bool, now: u64, progress: u64, allowance: u64) -> bool {
    changed || now.saturating_sub(progress) < allowance
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitStatus {
    Alive,
    Ended,
    Failed,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitAction {
    WatchAlive,
    WatchEnded { start_clock: bool },
    Fail,
}
/// Q43, state-table.md:800 at cdacce8f: only the outside owner's record ends a declared wait.
/// The first ending starts progress time once, preserved existing behaviour (no rule).
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match status { WaitStatus::Alive => WaitAction::WatchAlive, WaitStatus::Ended => WaitAction::WatchEnded { start_clock: !already_ended }, WaitStatus::Failed => WaitAction::Fail },
))]
pub fn wait_action(status: WaitStatus, already_ended: bool) -> WaitAction {
    match status {
        WaitStatus::Alive => WaitAction::WatchAlive,
        WaitStatus::Ended => WaitAction::WatchEnded {
            start_clock: !already_ended,
        },
        WaitStatus::Failed => WaitAction::Fail,
    }
}

/// Preserved existing behaviour (no rule): an announced quota never schedules before the sampled current time.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == if reset > now { reset } else { now },))]
pub fn quota_due(reset: u64, now: u64) -> u64 {
    if reset > now { reset } else { now }
}

/// State-table.md:796 at cdacce8f: either silence or no-progress allowance triggers a look. Saturation is preserved existing behaviour (no rule).
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == capped_add(now, if silence < no_progress { silence } else { no_progress }),
))]
pub fn progress_due(now: u64, silence: u64, no_progress: u64) -> u64 {
    now.saturating_add(if silence < no_progress {
        silence
    } else {
        no_progress
    })
}

/// Preserved existing behaviour (no rule): a model's recorded quota reset only increases.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == match previous { None => true, Some(old) => old < reset },))]
pub fn record_quota(previous: Option<u64>, reset: u64) -> bool {
    match previous {
        None => true,
        Some(old) => old < reset,
    }
}

/// Preserved existing behaviour (no rule): replaying a model's same/older ending never restarts its wait.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (same_model && match previous { None => false, Some(old) => old >= reset }),))]
pub fn quota_replayed(previous: Option<u64>, same_model: bool, reset: u64) -> bool {
    match previous {
        None => false,
        Some(old) => old >= reset && same_model,
    }
}
