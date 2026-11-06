//! Executed transition reports. Entries are separate from delivery outputs.
use super::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailedRoundCause {
    ChecksFailed,
    ChangesRequested,
    MakerCantMeet,
    CiFailed,
    RunFailed,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopCause {
    UserStop,
    Replaced,
    PrClosed,
    Merged,
    Duplicate,
    AlreadyDone,
    NotActionable,
    ReviewStop(ReviewStop),
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyKind {
    IngestDone,
    IngestEarlyEnd,
    AuditAgrees,
    AuditDisagrees,
    PlanLeaf,
    PlanSplit,
    PlanCantMeet,
    PlanCheckRejected,
    PlanCheckNeedsSplit,
    ProtectedPaths,
    MakerDone,
    MakerNeedsFiles,
    MakerCantMeet,
    MakerOutOfRoom,
    ChecksDone,
    Approved,
    ChangesRequested,
    ReviewerOutOfRoom,
    RebaseClean,
    RebaseNeedsChanges,
    RunFailed,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportEntry {
    FailedRound { node: NodeId, cause: FailedRoundCause },
    Stopped { node: NodeId, cause: StopCause },
    Stale { node: NodeId, reply_kind: ReplyKind },
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub entries: Vec<ReportEntry>,
}

#[cfg(verus_keep_ghost)]
verus! {
#[verifier::opaque]
pub closed spec fn forwards(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>,
    entries: Seq<ReportEntry>) -> bool {
    r1 == r0 + entries
}

#[verifier::opaque]
pub closed spec fn failed_round(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>,
    node: NodeId, cause: FailedRoundCause) -> bool {
    r1 == r0.push(ReportEntry::FailedRound { node, cause })
}

#[verifier::opaque]
pub closed spec fn stop_cause(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>,
    node: NodeId, before: Phase, after: Phase, cause: StopCause) -> bool {
    r1 == if !(before is Stopping || before is Closed)
        && (after is Stopping || after is Closed) {
        r0.push(ReportEntry::Stopped { node, cause })
    } else { r0 }
}

#[verifier::opaque]
pub closed spec fn stale_refusal(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>,
    node: NodeId, reply_kind: ReplyKind) -> bool {
    r1 == r0.push(ReportEntry::Stale { node, reply_kind })
}

#[verifier::opaque]
pub closed spec fn counter_exhausted(headroom: bool, refused: bool) -> bool {
    refused == !headroom
}
}

impl Report {
    kernel_fn! {
    pub fn new() -> (result: Self) [
        ensures result.entries@ == Seq::<ReportEntry>::empty(),
    ] {
        Self { entries: Vec::new() }
    }
    }

    kernel_fn! {
    pub(super) fn record_failed_round(&mut self, node: NodeId, cause: FailedRoundCause)
        -> (result: ()) [
        ensures failed_round(old(self).entries@, final(self).entries@, node, cause),
    ] {
        kernel_snapshot! { before = self.entries@ }
        self.entries.push(ReportEntry::FailedRound { node, cause });
        kernel_proof! { lemmas::append_failed_round(before@, self.entries@, node, cause); }
    }
    }

    kernel_fn! {
    pub(super) fn record_stop(&mut self, node: NodeId, before: Phase, after: Phase,
        cause: StopCause) -> (result: ()) [
        ensures stop_cause(old(self).entries@, final(self).entries@,
            node, before, after, cause),
    ] {
        kernel_snapshot! { entries = self.entries@ }
        if !before.is_stopping() && !before.is_closed()
            && (after.is_stopping() || after.is_closed()) {
            self.entries.push(ReportEntry::Stopped { node, cause });
        }
        kernel_proof! { lemmas::append_stop(entries@, self.entries@, node, before, after, cause); }
    }
    }

    kernel_fn! {
    pub(super) fn record_stale(&mut self, node: NodeId, reply_kind: ReplyKind)
        -> (result: ()) [
        ensures stale_refusal(old(self).entries@, final(self).entries@, node, reply_kind),
    ] {
        kernel_snapshot! { before = self.entries@ }
        self.entries.push(ReportEntry::Stale { node, reply_kind });
        kernel_proof! { lemmas::append_stale(before@, self.entries@, node, reply_kind); }
    }
    }
}

#[cfg(verus_keep_ghost)]
#[path = "../verus/report.rs"]
pub(super) mod lemmas;
