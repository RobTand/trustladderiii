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
    FailedRound {
        node: NodeId,
        cause: FailedRoundCause,
    },
    Stopped {
        node: NodeId,
        cause: StopCause,
    },
    Stale {
        node: NodeId,
        reply_kind: ReplyKind,
    },
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
    /// Verification-only observation of the actual input to settlement.
    #[cfg(verus_keep_ghost)]
    pub settlement_input: Ghost<(Seq<Phase>, Seq<ReportEntry>)>,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct StepRefusal {
    pub world: World,
    pub exhaustion: CounterExhaustion,
}

#[cfg(verus_keep_ghost)]
verus! {
/// Admission field group captured before applying the event.
pub struct ReportInput {
    pub phase: Phase,
    pub current: Option<Item>,
    pub run: Option<u64>,
    pub job: Option<u64>,
    pub accepted: bool,
}

pub open spec fn event_target(event: Event) -> Option<NodeId> {
    match event {
        Event::Reply { node, .. } | Event::Ack { node, .. }
        | Event::Fact { node, .. } | Event::Command { node, .. } => Some(node),
        _ => None,
    }
}

pub open spec fn user_stop(event: Event) -> bool {
    match event {
        Event::Command { command: Command::Stop, .. }
        | Event::Command { command: Command::Answer { answer: Answer::Stop, .. }, .. } => true,
        _ => false,
    }
}

#[verifier::opaque]
pub closed spec fn event_entries(event: Event, input: ReportInput) -> Seq<ReportEntry> {
    match event {
        Event::Reply { node, generation, reply } => reply_entries(node, input.phase,
            input.current, input.run, input.job, generation, reply, input.accepted),
        Event::Fact { node, fact } => round_entries(node, ci_cause(input.phase, input.current, fact)),
        _ => Seq::empty(),
    }
}

/// A stop's phase and review cause are observed together at the event transition.
/// Settlement can finish teardown but preserves that outcome and report entry.
pub open spec fn stop_trace(witness: (Phase, Option<ReviewStop>, Seq<ReportEntry>),
    event: Event, node: NodeId, before: Phase, after: Phase, input: ReportInput,
    r0: Seq<ReportEntry>, r1: Seq<ReportEntry>) -> bool {
    terminal_phase_frame(witness.0, after)
        && forwards(r0, witness.2, event_entries(event, input))
        && stop_cause(witness.2, r1, node, before, witness.0,
            stop_kind(witness.0, witness.1, false, user_stop(event)))
}

/// Exact event reports followed by settlement at its actual observed input.
/// Facts bind their failed-round cause to the pre-event phase and current item.
#[verifier::opaque]
pub closed spec fn agrees(event: Event,
    before: Seq<Phase>, after: Seq<Phase>, input: ReportInput,
    r0: Seq<ReportEntry>, r1: Seq<ReportEntry>,
    settlement_input: (Seq<Phase>, Seq<ReportEntry>)) -> bool {
    before.len() <= after.len() <= before.len() + 1
        && (forall|j: int| 0 <= j < before.len()
            && event_target(event) != Some(NodeId(j as u64))
            ==> terminal_phase_frame(before[j], after[j]))
        && (forall|j: int| before.len() <= j < after.len()
            ==> !(after[j] is Stopping || after[j] is Closed))
        && if let Event::Fact { node, fact } = event {
            settlement_input.0.len() == before.len()
                && settled_forwards(settlement_input.0, after, settlement_input.1, r1)
                && if node.0 < before.len() {
                    input.phase == before[node.0 as int]
                        && fact_report(r0, settlement_input.1, node, input.phase, input.current,
                            settlement_input.0[node.0 as int], fact)
                } else { forwards(r0, settlement_input.1, Seq::empty()) }
        } else { match event_target(event) {
            Some(node) if node.0 < before.len() =>
                exists|witness: (Phase, Option<ReviewStop>, Seq<ReportEntry>)|
                    stop_trace(witness, event, node, before[node.0 as int], after[node.0 as int], input, r0, r1),
            _ => forwards(r0, r1, Seq::empty()),
        } }
}

/// State-table CI rows: only a non-infrastructure failure at the queue boundary
/// consumes a failed round. The inputs are sampled before fact dispatch.
#[verifier::opaque]
pub closed spec fn ci_cause(phase: Phase, current: Option<Item>, fact: Fact)
    -> Option<FailedRoundCause> {
    match fact {
        Fact::CiFailed { infra: false } if phase == Phase::AwaitingDelivery
            || phase == Phase::Publishing && current == Some(Item::Do(EffectKind::Enqueue)) =>
            Some(FailedRoundCause::CiFailed),
        _ => None,
    }
}

/// Exact local report result, retaining the prefix and runtime entry order.
#[verifier::opaque]
pub(super) closed spec fn fact_record(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>,
    node: NodeId, before: Phase, after: Phase, fact: Fact, ci: Option<FailedRoundCause>) -> bool {
    let middle = if fact is CiFailed {
        match ci { Some(cause) => r0.push(ReportEntry::FailedRound { node, cause }), None => r0 }
    } else { r0 };
    let review = if fact is CiFailed { Some(ReviewStop::CiFailed) } else { None };
    stop_cause(middle, r1, node, before, after,
        stop_kind(after, review, fact is PrClosedByPerson, false))
}

/// Q18/CI/PrClosed rows: exact fact entries determined by the real pre-state.
#[verifier::opaque]
pub closed spec fn fact_report(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>,
    node: NodeId, before: Phase, current: Option<Item>, after: Phase, fact: Fact) -> bool {
    fact_record(r0, r1, node, before, after, fact, ci_cause(before, current, fact))
}

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
pub closed spec fn stop_kind(phase: Phase, review: Option<ReviewStop>,
    pr_closed: bool, user_stop: bool) -> StopCause {
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
pub closed spec fn reply_kind(reply: Reply) -> ReplyKind {
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
pub closed spec fn reply_entries(node: NodeId, phase: Phase, current: Option<Item>,
    run: Option<u64>, job: Option<u64>, generation: u64, reply: Reply, accepted: bool)
    -> Seq<ReportEntry> {
    if phase is Stopping { Seq::empty() }
    else if current != Some(Item::Await) || (run != Some(generation) && job != Some(generation)) {
        seq![ReportEntry::Stale { node, reply_kind: reply_kind(reply) }]
    } else if !accepted { round_entries(node, Some(FailedRoundCause::RunFailed)) }
    else { round_entries(node, work_failure(reply)) }
}

#[verifier::opaque]
pub closed spec fn terminal_phase_frame(before: Phase, after: Phase) -> bool {
    match before {
        Phase::Stopping(o) => after == Phase::Stopping(o) || after == Phase::Closed(o),
        Phase::Closed(o) => after == Phase::Closed(o),
        _ => !(after is Stopping || after is Closed),
    }
}

/// Stop-cause agreement for internal moves: settling never introduces a stop,
/// and completion of teardown preserves its existing outcome without a new entry.
#[verifier::opaque]
pub(super) closed spec fn no_new_stop(before: Phase, after: Phase,
    r0: Seq<ReportEntry>, r1: Seq<ReportEntry>) -> bool {
    terminal_phase_frame(before, after) && forwards(r0, r1, Seq::empty())
}

#[verifier::opaque]
pub(super) closed spec fn settled_forwards(before: Seq<Phase>, after: Seq<Phase>,
    r0: Seq<ReportEntry>, r1: Seq<ReportEntry>) -> bool {
    before.len() == after.len() && forwards(r0, r1, Seq::empty())
        && forall|i: int| 0 <= i < before.len() ==> terminal_phase_frame(before[i], after[i])
}

}

kernel_fn! {
impl Report {
    #[allow(clippy::new_without_default)]
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
        ensures result == stop_kind(phase, review, pr_closed, user_stop),
    ] {
        kernel_proof! { lemmas::stop_kind_value(phase, review, pr_closed, user_stop); }
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

kernel_fn! {
/// Fact reporting is isolated from the fact handlers' world frames.
#[allow(clippy::collapsible_if)]
pub(super) fn record_fact(report: &mut Report, node: NodeId, before: Phase, after: Phase,
    fact: Fact, ci: Option<FailedRoundCause>) -> (result: ()) [
    ensures fact_record(old(report).entries@, final(report).entries@, node, before, after, fact, ci),
] {
    kernel_snapshot! { entries_before = report.entries@ }
    if matches!(fact, Fact::CiFailed { .. }) {
        if let Some(cause) = ci {
            report.record_failed_round(node, cause);
            kernel_proof! { lemmas::failed_round_value(entries_before@, report.entries@, node, cause); }
        }
    }
    kernel_snapshot! { entries_middle = report.entries@ }
    let review = if matches!(fact, Fact::CiFailed { .. }) { Some(ReviewStop::CiFailed) } else { None };
    let cause = StopCause::from_phase(after, review, matches!(fact, Fact::PrClosedByPerson), false);
    report.record_stop(node, before, after, cause);
    kernel_proof! { lemmas::fact_record_result(entries_before@, entries_middle@, report.entries@,
        node, before, after, fact, ci); }
}
}

kernel_fn! {
/// State-table CI rows: sample the failed-round cause before fact mutation.
pub(super) fn ci_failure_cause(phase: Phase, current: Option<Item>, fact: Fact)
    -> (result: Option<FailedRoundCause>) [
    ensures result == ci_cause(phase, current, fact),
] {
    kernel_proof! { lemmas::ci_cause_value(phase, current, fact); }
    match fact {
        Fact::CiFailed { infra: false } if phase == Phase::AwaitingDelivery
            || phase == Phase::Publishing && current == Some(Item::Do(EffectKind::Enqueue)) => {
            Some(FailedRoundCause::CiFailed)
        }
        _ => None,
    }
}
}
