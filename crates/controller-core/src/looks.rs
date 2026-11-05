//! Allowance and incident decisions. The caller retains every clock and I/O sample.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncidentCause {
    Kernel { needs_rob: bool },
    ReadFailure { incidents: u64 },
    NoticeFailure { incidents: u64 },
}

/// Q7, state-table.md:270-275,309 at cdacce8f: later incidents require Rob.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match cause {
        IncidentCause::Kernel { needs_rob } => needs_rob,
        IncidentCause::ReadFailure { incidents } | IncidentCause::NoticeFailure { incidents } => incidents > 1,
    },
))]
pub fn incident_needs_rob(cause: IncidentCause) -> bool {
    match cause {
        IncidentCause::Kernel { needs_rob } => needs_rob,
        IncidentCause::ReadFailure { incidents } | IncidentCause::NoticeFailure { incidents } => {
            incidents > 1
        }
    }
}

/// Q7, state-table.md:272-275: a probe cannot clear a closed or Rob-only incident.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (open && !needs_rob),
))]
pub fn probe_allowed(open: bool, needs_rob: bool) -> bool {
    open && !needs_rob
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IncidentView {
    pub needs_rob: bool,
    pub refused_command: bool,
    pub own_effect: bool,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncidentAction {
    Continue,
    Hold,
    Probe,
}

/// Q7, state-table.md:272-275,577 / #608: maintenance probes recoverable
/// incidents; a refused command holds; only this effect's incident intercepts a look.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    (out == IncidentAction::Probe) == (!incident.needs_rob && (maintenance || (!incident.refused_command && incident.own_effect))),
    (out == IncidentAction::Continue) == (!maintenance && !incident.refused_command && !incident.own_effect),
    (out == IncidentAction::Hold) == !( (!incident.needs_rob && (maintenance || (!incident.refused_command && incident.own_effect))) || (!maintenance && !incident.refused_command && !incident.own_effect) ),
))]
pub fn incident_action(maintenance: bool, incident: IncidentView) -> IncidentAction {
    if maintenance {
        if incident.needs_rob {
            IncidentAction::Hold
        } else {
            IncidentAction::Probe
        }
    } else if incident.refused_command {
        IncidentAction::Hold
    } else if incident.own_effect {
        if incident.needs_rob {
            IncidentAction::Hold
        } else {
            IncidentAction::Probe
        }
    } else {
        IncidentAction::Continue
    }
}

/// #403 / Q7, state-table.md:270-275: a postmortem never raises an incident.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == !postmortem,))]
pub fn incident_for_operation(postmortem: bool) -> bool {
    !postmortem
}

/// Q7, state-table.md:309: an incident notice never escalates itself.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == !notice,))]
pub fn incident_for_notice(notice: bool) -> bool {
    !notice
}

/// #356 / Q7, state-table.md:270-275: failures count once per silence period.
/// A backwards clock has zero elapsed time, as in the original saturating subtraction.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match last {
        None => true,
        Some(previous) => if now >= previous { now - previous >= period } else { period == 0 },
    },
))]
pub fn count_failure(last: Option<u64>, now: u64, period: u64) -> bool {
    match last {
        None => true,
        Some(previous) => now.saturating_sub(previous) >= period,
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FailureCount {
    pub failures: u8,
    pub incidents: u8,
    pub raise: bool,
}

/// Q7, state-table.md:270-275,309: every second counted failure raises, per effect.
/// The shell owns one pair of counters per allowance. The old u8 increment also
/// requires headroom; the reachable stored counter is zero or one.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => requires failures < 255, ensures
    out.raise == (failures >= 1),
    out.failures == (if failures == 0 { 1u8 } else { 0u8 }),
    out.incidents as int == (if failures == 0 || incidents == 255 { incidents as int } else { incidents as int + 1 }),
))]
pub fn counted_failure(failures: u8, incidents: u8) -> FailureCount {
    let failures = failures + 1;
    if failures < 2 {
        return FailureCount {
            failures,
            incidents,
            raise: false,
        };
    }
    FailureCount {
        failures: 0,
        incidents: incidents.saturating_add(1),
        raise: true,
    }
}

/// State-table.md:270-275 / #356: rearming uses the already sampled clock.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out as int == if (now as int) + (delay as int) > u64::MAX as int { u64::MAX as int } else { (now as int) + (delay as int) },
))]
pub fn rearm_at(now: u64, delay: u64) -> u64 {
    now.saturating_add(delay)
}

/// State-table.md:577, state-machine.md:442: progress allowance decides a stall.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (if now >= progress { now - progress >= allowance } else { allowance == 0 }),
))]
pub fn stalled(now: u64, progress: u64, allowance: u64) -> bool {
    now.saturating_sub(progress) >= allowance
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StallUpstream {
    Workers,
    GitHub,
    Mergify,
    PrismaBuild,
    Worktrees,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StallAction {
    StopWorker,
    Requeue,
    Unfinished,
    Ignore,
}

/// State-table.md:577 / state-machine.md:442 / #356: stop a stalled worker;
/// report a GitHub/Mergify stall with its queue class. Other owners retain progress.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    (out == StallAction::StopWorker) == (source == StallUpstream::Workers),
    (out == StallAction::Requeue) == ((source == StallUpstream::GitHub || source == StallUpstream::Mergify) && requeue),
    (out == StallAction::Unfinished) == ((source == StallUpstream::GitHub || source == StallUpstream::Mergify) && !requeue),
    (out == StallAction::Ignore) == (source == StallUpstream::PrismaBuild || source == StallUpstream::Worktrees),
))]
pub fn stall_action(source: StallUpstream, requeue: bool) -> StallAction {
    match source {
        StallUpstream::Workers => StallAction::StopWorker,
        StallUpstream::GitHub | StallUpstream::Mergify => {
            if requeue {
                StallAction::Requeue
            } else {
                StallAction::Unfinished
            }
        }
        StallUpstream::PrismaBuild | StallUpstream::Worktrees => StallAction::Ignore,
    }
}

/// Q7, state-table.md:270-275,309: mutate only the selected effect's counters.
/// The mutable argument makes the per-effect frame explicit to the verifier.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires old(state).failures < 255,
    ensures state.raise == (old(state).failures >= 1),
        state.failures == (if old(state).failures == 0 { 1u8 } else { 0u8 }),
        state.incidents as int == (if old(state).failures == 0 || old(state).incidents == 255 { old(state).incidents as int } else { old(state).incidents as int + 1 }),
))]
pub fn record_failure(state: &mut FailureCount) {
    *state = counted_failure(state.failures, state.incidents);
}

/// Q7, state-table.md:270-275: an open incident blocks another read incident.
/// The kernel's distinct :577 guard and transition are exercised by the real kernel in the I/O test.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    open ==> !out,
    !open ==> out,
))]
pub fn incident_allows_failure(open: bool) -> bool {
    !open
}
