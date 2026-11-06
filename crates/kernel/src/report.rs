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
#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub entries: Vec<ReportEntry>,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct StepResult {
    pub world: World,
    pub outputs: Vec<Out>,
    pub report: Report,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct StepRefusal {
    pub world: World,
    pub exhaustion: CounterExhaustion,
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

#[verifier::opaque]
pub(super) closed spec fn round_entries(node: NodeId, cause: Option<FailedRoundCause>) -> Seq<ReportEntry> {
    match cause {
        Some(cause) => seq![ReportEntry::FailedRound { node, cause }],
        None => Seq::empty(),
    }
}

#[verifier::opaque]
pub(super) closed spec fn failure_cause(step: ReviewStop) -> FailedRoundCause {
    match step {
        ReviewStop::ChecksFailed => FailedRoundCause::ChecksFailed,
        ReviewStop::ChangesRequested => FailedRoundCause::ChangesRequested,
        ReviewStop::CiFailed => FailedRoundCause::CiFailed,
        _ => FailedRoundCause::MakerCantMeet,
    }
}

#[verifier::opaque]
pub(super) closed spec fn work_failure(reply: Reply) -> Option<FailedRoundCause> {
    match reply {
        Reply::MakerCantMeet => Some(FailedRoundCause::MakerCantMeet),
        Reply::ChecksDone { pass, in_footprint, .. } => if pass && in_footprint {
            None
        } else { Some(FailedRoundCause::ChecksFailed) },
        Reply::ChangesRequested => Some(FailedRoundCause::ChangesRequested),
        _ => None,
    }
}

#[verifier::opaque]
pub(super) closed spec fn ci_failure(phase: Phase, current: Option<Item>, infra: bool)
    -> Option<FailedRoundCause> {
    if !infra && (phase == Phase::Queued
        || phase == Phase::Publishing && current == Some(Item::Do(EffectKind::Enqueue))) {
        Some(FailedRoundCause::CiFailed)
    } else { None }
}

#[verifier::opaque]
pub(super) closed spec fn fact_failure(phase: Phase, current: Option<Item>, fact: Fact)
    -> Option<FailedRoundCause> {
    match fact { Fact::CiFailed { infra } => ci_failure(phase, current, infra), _ => None }
}
#[verifier::opaque]
pub(super) closed spec fn reply_kind(reply: Reply) -> ReplyKind {
match reply {
            Reply::IngestDone { .. } => ReplyKind::IngestDone,
            Reply::IngestEarlyEnd(_) => ReplyKind::IngestEarlyEnd,
            Reply::AuditAgrees => ReplyKind::AuditAgrees,
            Reply::AuditDisagrees => ReplyKind::AuditDisagrees,
            Reply::PlanLeaf { .. } => ReplyKind::PlanLeaf,
            Reply::PlanSplit => ReplyKind::PlanSplit,
            Reply::PlanCantMeet => ReplyKind::PlanCantMeet,
            Reply::PlanCheckRejected => ReplyKind::PlanCheckRejected,
            Reply::PlanCheckNeedsSplit => ReplyKind::PlanCheckNeedsSplit,
            Reply::ProtectedPaths => ReplyKind::ProtectedPaths,
            Reply::MakerDone { .. } => ReplyKind::MakerDone,
            Reply::MakerNeedsFiles { .. } => ReplyKind::MakerNeedsFiles,
            Reply::MakerCantMeet => ReplyKind::MakerCantMeet,
            Reply::MakerOutOfRoom => ReplyKind::MakerOutOfRoom,
            Reply::ChecksDone { .. } => ReplyKind::ChecksDone,
            Reply::Approved { .. } => ReplyKind::Approved,
            Reply::ChangesRequested => ReplyKind::ChangesRequested,
            Reply::ReviewerOutOfRoom => ReplyKind::ReviewerOutOfRoom,
            Reply::RebaseClean { .. } => ReplyKind::RebaseClean,
            Reply::RebaseNeedsChanges => ReplyKind::RebaseNeedsChanges,
            Reply::RunFailed => ReplyKind::RunFailed,
        }
}

#[verifier::opaque]
pub(super) closed spec fn reply_entries(node: NodeId, phase: Phase, current: Option<Item>,
    run: Option<u64>, job: Option<u64>, generation: u64, reply: Reply, accepted: bool)
    -> Seq<ReportEntry> {
    if phase is Stopping { Seq::empty() }
    else if current != Some(Item::Await) || (run != Some(generation) && job != Some(generation)) {
        seq![ReportEntry::Stale { node, reply_kind: reply_kind(reply) }]
    } else if !accepted { round_entries(node, Some(FailedRoundCause::RunFailed)) }
    else { round_entries(node, work_failure(reply)) }
}

}

kernel_fn! {
impl Report {
    pub fn new() -> (result: Self) [
        ensures result.entries@ == Seq::<ReportEntry>::empty(),
    ] {
        Self { entries: Vec::new() }
    }
}
}

impl Report {
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

kernel_fn! {
impl ReplyKind {
    pub fn of(reply: Reply) -> (result: Self) [
        ensures result == reply_kind(reply),
    ] {
        kernel_proof! { lemmas::kind_value(reply); }
        match reply {
            Reply::IngestDone { .. } => ReplyKind::IngestDone,
            Reply::IngestEarlyEnd(_) => ReplyKind::IngestEarlyEnd,
            Reply::AuditAgrees => ReplyKind::AuditAgrees,
            Reply::AuditDisagrees => ReplyKind::AuditDisagrees,
            Reply::PlanLeaf { .. } => ReplyKind::PlanLeaf,
            Reply::PlanSplit => ReplyKind::PlanSplit,
            Reply::PlanCantMeet => ReplyKind::PlanCantMeet,
            Reply::PlanCheckRejected => ReplyKind::PlanCheckRejected,
            Reply::PlanCheckNeedsSplit => ReplyKind::PlanCheckNeedsSplit,
            Reply::ProtectedPaths => ReplyKind::ProtectedPaths,
            Reply::MakerDone { .. } => ReplyKind::MakerDone,
            Reply::MakerNeedsFiles { .. } => ReplyKind::MakerNeedsFiles,
            Reply::MakerCantMeet => ReplyKind::MakerCantMeet,
            Reply::MakerOutOfRoom => ReplyKind::MakerOutOfRoom,
            Reply::ChecksDone { .. } => ReplyKind::ChecksDone,
            Reply::Approved { .. } => ReplyKind::Approved,
            Reply::ChangesRequested => ReplyKind::ChangesRequested,
            Reply::ReviewerOutOfRoom => ReplyKind::ReviewerOutOfRoom,
            Reply::RebaseClean { .. } => ReplyKind::RebaseClean,
            Reply::RebaseNeedsChanges => ReplyKind::RebaseNeedsChanges,
            Reply::RunFailed => ReplyKind::RunFailed,
        }
    }
}
}

kernel_fn! {
impl StopCause {
    pub(super) fn from_phase(phase: Phase, review: Option<ReviewStop>,
        pr_closed: bool, user_stop: bool) -> (result: Self) [
        ensures result == match phase {
            Phase::Stopping(Outcome::Merged) | Phase::Closed(Outcome::Merged) => StopCause::Merged,
            Phase::Stopping(Outcome::Replaced) | Phase::Closed(Outcome::Replaced) => StopCause::Replaced,
            Phase::Stopping(Outcome::Duplicate) | Phase::Closed(Outcome::Duplicate) => StopCause::Duplicate,
            Phase::Stopping(Outcome::AlreadyDone) | Phase::Closed(Outcome::AlreadyDone) => StopCause::AlreadyDone,
            Phase::Stopping(Outcome::NotActionable) | Phase::Closed(Outcome::NotActionable) => StopCause::NotActionable,
            _ => if pr_closed { StopCause::PrClosed } else if user_stop { StopCause::UserStop }
                else { match review { Some(kind) => StopCause::ReviewStop(kind), None => StopCause::UserStop } },
        },
    ] {
        match phase {
            Phase::Stopping(Outcome::Merged) | Phase::Closed(Outcome::Merged) => StopCause::Merged,
            Phase::Stopping(Outcome::Replaced) | Phase::Closed(Outcome::Replaced) => StopCause::Replaced,
            Phase::Stopping(Outcome::Duplicate) | Phase::Closed(Outcome::Duplicate) => StopCause::Duplicate,
            Phase::Stopping(Outcome::AlreadyDone) | Phase::Closed(Outcome::AlreadyDone) => StopCause::AlreadyDone,
            Phase::Stopping(Outcome::NotActionable) | Phase::Closed(Outcome::NotActionable) => StopCause::NotActionable,
            _ => if pr_closed { StopCause::PrClosed } else if user_stop { StopCause::UserStop }
                else { match review { Some(kind) => StopCause::ReviewStop(kind), None => StopCause::UserStop } },
        }
    }
}
}
