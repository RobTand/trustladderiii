//! H19 decisions. Files, clocks, tools, and durable ordering belong to the shell.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Absent,
    Moving,
    Live,
    Ended,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Withdrawal {
    MarkGone,
    Pending,
    Withdraw,
    ReadEnding,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Complete,
    Waiting,
    Unreadable,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckResult {
    Confirmed,
    Absent,
    Pending,
    Unreadable,
}

/// State-table delivery fencing. The shell constructs this only after fsync.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fence {
    epoch: i64,
    accepted: i64,
}

#[cfg(verus_keep_ghost)]
verus! {
    pub closed spec fn fence_valid(token: Fence) -> bool {
        token.epoch >= token.accepted
    }
    pub closed spec fn fence_matches(token: Fence, epoch: i64, accepted: i64) -> bool {
        token.epoch == epoch && token.accepted == accepted
    }
}

/// State-table delivery epochs, unbounded over the complete i64 domain.
#[cfg_attr(verus_keep_ghost, verus_spec(out => ensures
    out.is_some() == (epoch >= accepted),
    out.is_some() ==> fence_matches(out.unwrap(), epoch, accepted),
))]
pub fn fence(epoch: i64, accepted: i64) -> Option<Fence> {
    if epoch < accepted {
        None
    } else {
        Some(Fence { epoch, accepted })
    }
}

/// Submit a PrismaBuild job: Once by action key. Unbounded; no retry counter.
/// A capability is returned to the caller only after the shell's fence fsync.
#[cfg_attr(verus_keep_ghost, verus_spec(out => ensures
    out == (matches!(record, Record::Absent) && fence_valid(fence)),
    out ==> fence_valid(fence),
))]
pub fn publish(record: Record, fence: Fence) -> bool {
    matches!(record, Record::Absent) && fence.epoch >= fence.accepted
}

/// #369 / Q43: moving and queued/claimed evidence is live, without a clock.
#[cfg_attr(verus_keep_ghost, verus_spec(out => ensures
    out == (matches!(record, Record::Moving) || matches!(record, Record::Live)),
))]
pub fn live(record: Record) -> bool {
    matches!(record, Record::Moving | Record::Live)
}

/// Withdraw a PrismaBuild job: Converges, finished or unknown counts as gone.
/// Ended records still read the trusted ending at the shell's original point.
#[cfg_attr(verus_keep_ghost, verus_spec(out => ensures
    matches!(out, Withdrawal::MarkGone) == matches!(record, Record::Absent),
    matches!(out, Withdrawal::Pending) == matches!(record, Record::Moving),
    matches!(out, Withdrawal::Withdraw) == matches!(record, Record::Live),
    matches!(out, Withdrawal::ReadEnding) == matches!(record, Record::Ended),
))]
pub fn withdrawal(record: Record) -> Withdrawal {
    match record {
        Record::Absent => Withdrawal::MarkGone,
        Record::Moving => Withdrawal::Pending,
        Record::Live => Withdrawal::Withdraw,
        Record::Ended => Withdrawal::ReadEnding,
    }
}

/// Submit reconciliation / Q43. Total over the typed record/ending domains.
/// Unknown or malformed endings are Unreadable, never Confirmed. Unbounded.
#[cfg_attr(verus_keep_ghost, verus_spec(out => ensures
    matches!(out, CheckResult::Confirmed) ==
        (!matches!(record, Record::Moving | Record::Live) && matches!(ending, Ending::Complete)),
    matches!(ending, Ending::Unreadable) ==> !matches!(out, CheckResult::Confirmed),
    matches!(out, CheckResult::Absent) ==
        (matches!(record, Record::Absent) && matches!(ending, Ending::Waiting)),
    matches!(out, CheckResult::Unreadable) ==
        (!matches!(record, Record::Moving | Record::Live) && matches!(ending, Ending::Unreadable)),
    matches!(out, CheckResult::Pending) ==
        (matches!(record, Record::Moving | Record::Live) ||
         matches!(record, Record::Ended) && matches!(ending, Ending::Waiting)),
))]
pub fn check_result(record: Record, ending: Ending) -> CheckResult {
    if live(record) {
        return CheckResult::Pending;
    }
    match ending {
        Ending::Complete => CheckResult::Confirmed,
        Ending::Unreadable => CheckResult::Unreadable,
        Ending::Waiting => match record {
            Record::Absent => CheckResult::Absent,
            _ => CheckResult::Pending,
        },
    }
}

/// H19 plan rejection never replaces an existing action-key binding.
#[cfg_attr(verus_keep_ghost, verus_spec(out => ensures out == !bound))]
pub fn inspect_plan(bound: bool) -> bool {
    !bound
}

/// Stopping: only jobs in the explicitly withdrawn node generation are targets.
#[cfg_attr(verus_keep_ghost, verus_spec(out => ensures
    out == (job_node == node && generation == Some(job_generation)),
))]
pub fn target(job_node: u64, node: u64, job_generation: u64, generation: Option<u64>) -> bool {
    job_node == node && generation == Some(job_generation)
}

/// Closed is final / delivery watches. Retire only after effects and watches settle.
#[cfg_attr(verus_keep_ghost, verus_spec(out => ensures out == (closed && !referenced)))]
pub fn retire(closed: bool, referenced: bool) -> bool {
    closed && !referenced
}

/// H19 durable completion supersedes only strictly older job generations.
#[cfg_attr(verus_keep_ghost, verus_spec(out => ensures out == (old < new)))]
pub fn superseded(old: u64, new: u64) -> bool {
    old < new
}

/// Q43 subscription persistence: equal bytes produce no write or notification.
#[cfg_attr(verus_keep_ghost, verus_spec(out => ensures out == !equal))]
pub fn write_wait(equal: bool) -> bool {
    !equal
}

#[cfg(verus_keep_ghost)]
verus! {
    /// Submit Once: one restart step. PB's authentic durable record survives
    /// publication and the durable Job binding retains its original action key.
    /// Reapplying this step covers an unbounded number of crashes.
    fn once_after_crash(record: Record, fenced: Fence)
        requires !matches!(record, Record::Absent)
    {
        let again = publish(record, fenced);
        assert(!again);
    }
    /// Once anti-vacuity: absent can publish and every surviving state blocks it.
    fn once_witness() {
        let token = fence(1,1).unwrap();
        let absent = publish(Record::Absent, token);
        let moving = publish(Record::Moving, token);
        let live = publish(Record::Live, token);
        let ended = publish(Record::Ended, token);
        assert(absent && !moving && !live && !ended);
    }
    /// Withdraw Converges, one-step progress for gone jobs, no finite job bound.
    fn withdrawal_converges(record: Record)
        requires matches!(record, Record::Absent | Record::Ended)
    {
        let next = withdrawal(record);
        assert(!matches!(next, Withdrawal::Pending | Withdrawal::Withdraw));
    }
    /// Withdraw witness includes both absent and finished, plus trusted ending.
    fn withdrawal_witness() {
        let absent = withdrawal(Record::Absent);
        let ended = withdrawal(Record::Ended);
        let result = check_result(Record::Ended, Ending::Complete);
        assert(matches!(absent, Withdrawal::MarkGone));
        assert(matches!(ended, Withdrawal::ReadEnding));
        assert(matches!(result, CheckResult::Confirmed));
    }
    /// Submit reconciliation, complete record/ending domains, unbounded.
    fn unknown_ending(record: Record) {
        let result = check_result(record, Ending::Unreadable);
        assert(!matches!(result, CheckResult::Confirmed));
    }
    /// Result anti-vacuity covers each reconciliation and the unknown error.
    fn result_witness() {
        let found = check_result(Record::Ended, Ending::Complete);
        let absent = check_result(Record::Absent, Ending::Waiting);
        let pending = check_result(Record::Ended, Ending::Waiting);
        let unknown = check_result(Record::Ended, Ending::Unreadable);
        assert(matches!(found, CheckResult::Confirmed));
        assert(matches!(absent, CheckResult::Absent));
        assert(matches!(pending, CheckResult::Pending));
        assert(matches!(unknown, CheckResult::Unreadable));
    }
    /// Delivery fencing, full i64 domain. The shell persists before returning token.
    fn fence_before_submit(record: Record, token: Fence) {
        let publish = publish(record, token);
        assert(publish ==> token.epoch >= token.accepted);
    }
    /// Fence anti-vacuity: stale refuses, equal epoch allows publication.
    fn fence_witness() {
        let stale = fence(0,1);
        let accepted = fence(1,1);
        assert(stale.is_none() && accepted.is_some());
        let publication = publish(Record::Absent, accepted.unwrap());
        assert(publication);
    }
}
#[cfg(verus_keep_ghost)]
fn main() {}
