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
}
