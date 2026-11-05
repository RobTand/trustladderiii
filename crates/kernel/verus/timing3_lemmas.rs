//! #771 ingest arithmetic and Q30 holding/resume cases, separate from mutable frames.
use super::*;

verus! {
/// Ingest deposit row / #771: every u8 size plus the fixed reserve fits the ledger.
pub(super) proof fn ingest_deposit_bound(size: u8)
    ensures 3 <= size as int + RESERVE as int <= 258,
{
}

/// Q30 pause row / #771: the real resume match is total exactly for holding phases.
pub(super) proof fn pause_resume(phase: Phase)
    ensures (match phase {
        Phase::Making => Some(Resume::Making),
        Phase::Checking => Some(Resume::Checking),
        Phase::Reviewing => Some(Resume::Reviewing),
        Phase::Publishing | Phase::Queued => Some(Resume::Publishing),
        Phase::Rebasing => Some(Resume::Rebasing),
        _ => None,
    }) == if holding(phase) {
        Some(match phase {
            Phase::Making => Resume::Making,
            Phase::Checking => Resume::Checking,
            Phase::Reviewing => Resume::Reviewing,
            Phase::Publishing | Phase::Queued => Resume::Publishing,
            _ => Resume::Rebasing,
        })
    } else { None },
{
}
}
