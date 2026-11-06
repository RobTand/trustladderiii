use super::*;

verus! {
pub(in super::super) proof fn read_failed_round(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>,
    node: NodeId, cause: FailedRoundCause)
    requires failed_round(r0, r1, node, cause),
    ensures r1 == r0.push(ReportEntry::FailedRound { node, cause }),
{
    reveal(failed_round);
}

pub(in super::super) proof fn read_forwards(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>, entries: Seq<ReportEntry>)
    requires forwards(r0, r1, entries),
    ensures r1 == r0 + entries,
{
    reveal(forwards);
}

pub(in super::super) proof fn forward_entries(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>, entries: Seq<ReportEntry>)
    requires r1 == r0 + entries,
    ensures forwards(r0, r1, entries),
{
    reveal(forwards);
}

pub(in super::super) proof fn round_shape(node: NodeId, cause: Option<FailedRoundCause>)
    ensures round_entries(node, cause) == match cause {
        Some(cause) => seq![ReportEntry::FailedRound { node, cause }],
        None => Seq::empty(),
    },
{
    reveal(round_entries);
}

pub(in super::super) proof fn failure_kind(step: ReviewStop)
    ensures failure_cause(step) == match step {
        ReviewStop::ChecksFailed => FailedRoundCause::ChecksFailed,
        ReviewStop::ChangesRequested => FailedRoundCause::ChangesRequested,
        ReviewStop::CiFailed => FailedRoundCause::CiFailed,
        _ => FailedRoundCause::MakerCantMeet,
    },
{
    reveal(failure_cause);
}

pub(in super::super) proof fn work_kind(reply: Reply)
    ensures work_failure(reply) == match reply {
        Reply::MakerCantMeet => Some(FailedRoundCause::MakerCantMeet),
        Reply::ChecksDone { pass, in_footprint, .. } => if pass && in_footprint {
            None
        } else { Some(FailedRoundCause::ChecksFailed) },
        Reply::ChangesRequested => Some(FailedRoundCause::ChangesRequested),
        _ => None,
    },
{
    reveal(work_failure);
}

pub(in super::super) proof fn ci_kind(phase: Phase, current: Option<Item>, infra: bool)
    ensures ci_failure(phase, current, infra) == if !infra && (phase == Phase::Queued
        || phase == Phase::Publishing && current == Some(Item::Do(EffectKind::Enqueue))) {
        Some(FailedRoundCause::CiFailed)
    } else { None },
{
    reveal(ci_failure);
}

pub(in super::super) proof fn fact_kind(phase: Phase, current: Option<Item>, fact: Fact)
    ensures fact_failure(phase, current, fact) == match fact {
        Fact::CiFailed { infra } => ci_failure(phase, current, infra), _ => None,
    },
{
    reveal(fact_failure);
}

pub(in super::super) proof fn round_delta(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>, node: NodeId,
    cause: Option<FailedRoundCause>)
    requires forwards(r0, r1, round_entries(node, cause)),
    ensures r1 == match cause {
        Some(cause) => r0.push(ReportEntry::FailedRound { node, cause }),
        None => r0,
    },
{
    reveal(forwards);
    reveal(round_entries);
    assert(r0 + Seq::empty() =~= r0);
    if let Some(cause) = cause {
        assert(r0 + seq![ReportEntry::FailedRound { node, cause }]
            =~= r0.push(ReportEntry::FailedRound { node, cause }));
    }
}

pub(in super::super) proof fn forward_round(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>, node: NodeId,
    cause: Option<FailedRoundCause>)
    requires r1 == match cause {
        Some(cause) => r0.push(ReportEntry::FailedRound { node, cause }),
        None => r0,
    },
    ensures forwards(r0, r1, round_entries(node, cause)),
{
    reveal(forwards);
    reveal(round_entries);
    reveal(failed_round);
    assert(r0 + Seq::empty() =~= r0);
    if let Some(cause) = cause {
        assert(r0 + seq![ReportEntry::FailedRound { node, cause }]
            =~= r0.push(ReportEntry::FailedRound { node, cause }));
    }
}

pub(in super::super) proof fn unchanged(entries: Seq<ReportEntry>)
    ensures forwards(entries, entries, Seq::empty()),
{
    reveal(forwards);
    assert(entries + Seq::empty() =~= entries);
}

pub(in super::super) proof fn refusal(headroom: bool, refused: bool)
    requires refused == !headroom,
    ensures counter_exhausted(headroom, refused),
{
    reveal(counter_exhausted);
}

pub(in super::super) proof fn append_failed_round(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>,
    node: NodeId, cause: FailedRoundCause)
    requires r1 == r0.push(ReportEntry::FailedRound { node, cause }),
    ensures failed_round(r0, r1, node, cause),
{
    reveal(failed_round);
}

pub(in super::super) proof fn append_stop(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>, node: NodeId,
    before: Phase, after: Phase, cause: StopCause)
    requires r1 == if !(before is Stopping || before is Closed)
        && (after is Stopping || after is Closed) {
        r0.push(ReportEntry::Stopped { node, cause })
    } else { r0 },
    ensures stop_cause(r0, r1, node, before, after, cause),
{
    reveal(stop_cause);
}

pub(in super::super) proof fn append_stale(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>,
    node: NodeId, reply_kind: ReplyKind)
    requires r1 == r0.push(ReportEntry::Stale { node, reply_kind }),
    ensures stale_refusal(r0, r1, node, reply_kind),
{
    reveal(stale_refusal);
}
pub(in super::super) proof fn kind_value(reply: Reply)
    ensures reply_kind(reply) ==
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
        },
{ reveal(reply_kind); }

pub(in super::super) proof fn read_stale(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>,
    node: NodeId, kind: ReplyKind)
    requires stale_refusal(r0, r1, node, kind),
    ensures r1 == r0.push(ReportEntry::Stale { node, reply_kind: kind }),
{ reveal(stale_refusal); }

pub(in super::super) proof fn reply_result(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>,
    node: NodeId, phase: Phase, current: Option<Item>, run: Option<u64>, job: Option<u64>,
    generation: u64, reply: Reply, accepted: bool)
    requires r1 == if phase is Stopping { r0 }
        else if current != Some(Item::Await) || (run != Some(generation) && job != Some(generation)) {
            r0.push(ReportEntry::Stale { node, reply_kind: reply_kind(reply) })
        } else if !accepted { r0.push(ReportEntry::FailedRound { node, cause: FailedRoundCause::RunFailed }) }
        else { r0 + round_entries(node, work_failure(reply)) },
    ensures forwards(r0, r1, reply_entries(node, phase, current, run, job, generation, reply, accepted)),
{
    reveal(reply_entries);
    reveal(forwards);
    round_shape(node, Some(FailedRoundCause::RunFailed));
    assert(r0 + Seq::empty() =~= r0);
    assert(r0 + seq![ReportEntry::Stale { node, reply_kind: reply_kind(reply) }]
        =~= r0.push(ReportEntry::Stale { node, reply_kind: reply_kind(reply) }));
    assert(r0 + seq![ReportEntry::FailedRound { node, cause: FailedRoundCause::RunFailed }]
        =~= r0.push(ReportEntry::FailedRound { node, cause: FailedRoundCause::RunFailed }));
}

pub(in super::super) proof fn quiet(before: Phase, after: Phase, entries: Seq<ReportEntry>)
    requires match before {
        Phase::Stopping(o) => after == Phase::Stopping(o) || after == Phase::Closed(o),
        Phase::Closed(o) => after == Phase::Closed(o),
        _ => !(after is Stopping || after is Closed),
    },
    ensures no_new_stop(before, after, entries, entries),
{
    reveal(no_new_stop);
    reveal(terminal_phase_frame);
    unchanged(entries);
}

pub(in super::super) proof fn read_quiet(before: Phase, after: Phase,
    r0: Seq<ReportEntry>, r1: Seq<ReportEntry>)
    requires no_new_stop(before, after, r0, r1),
    ensures r1 == r0, terminal_phase_frame(before, after),
{
    reveal(no_new_stop);
    read_forwards(r0, r1, Seq::empty());
    assert(r0 + Seq::empty() =~= r0);
}

pub(in super::super) proof fn phase_refl(phase: Phase)
    ensures terminal_phase_frame(phase, phase),
{ reveal(terminal_phase_frame); }

pub(in super::super) proof fn phase_trans(before: Phase, middle: Phase, after: Phase)
    requires terminal_phase_frame(before, middle), terminal_phase_frame(middle, after),
    ensures terminal_phase_frame(before, after),
{ reveal(terminal_phase_frame); }

pub(in super::super) proof fn settle_result(before: Seq<Phase>, after: Seq<Phase>,
    r0: Seq<ReportEntry>, r1: Seq<ReportEntry>)
    requires before.len() == after.len(), r1 == r0,
        forall|i: int| 0 <= i < before.len() ==> terminal_phase_frame(before[i], after[i]),
    ensures settled_forwards(before, after, r0, r1),
{ reveal(settled_forwards); unchanged(r0); }

pub(in super::super) proof fn stop_kind_value(phase: Phase, review: Option<ReviewStop>,
    pr_closed: bool, user_stop: bool)
    ensures stop_kind(phase, review, pr_closed, user_stop) == match phase {
            Phase::Stopping(Outcome::Merged) | Phase::Closed(Outcome::Merged) => StopCause::Merged,
            Phase::Stopping(Outcome::Replaced) | Phase::Closed(Outcome::Replaced) => StopCause::Replaced,
            Phase::Stopping(Outcome::Duplicate) | Phase::Closed(Outcome::Duplicate) => StopCause::Duplicate,
            Phase::Stopping(Outcome::AlreadyDone) | Phase::Closed(Outcome::AlreadyDone) => StopCause::AlreadyDone,
            Phase::Stopping(Outcome::NotActionable) | Phase::Closed(Outcome::NotActionable) => StopCause::NotActionable,
            _ => if pr_closed { StopCause::PrClosed } else if user_stop { StopCause::UserStop }
                else { match review { Some(kind) => StopCause::ReviewStop(kind), None => StopCause::UserStop } },
        },
{ reveal(stop_kind); }

pub(in super::super) proof fn stop_agreement(r0: Seq<ReportEntry>, middle: Seq<ReportEntry>,
    r1: Seq<ReportEntry>, node: NodeId, before: Phase, after: Phase, cause: StopCause,
    entries: Seq<ReportEntry>, ci_gap: bool)
    requires ci_gap || forwards(r0, middle, entries),
        stop_cause(middle, r1, node, before, after, cause),
    ensures agrees_except_ci_failed(r0, r1, node, before, after, cause, entries, ci_gap),
{
    reveal(agrees_except_ci_failed);
    assert((ci_gap || forwards(r0, middle, entries))
        && stop_cause(middle, r1, node, before, after, cause));
    assert(exists|m: Seq<ReportEntry>| (ci_gap || forwards(r0, m, entries))
        && #[trigger] stop_cause(m, r1, node, before, after, cause));
}

}
