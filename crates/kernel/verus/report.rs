use super::*;

verus! {
pub proof fn append_failed_round(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>,
    node: NodeId, cause: FailedRoundCause)
    requires r1 == r0.push(ReportEntry::FailedRound { node, cause }),
    ensures failed_round(r0, r1, node, cause),
{
    reveal(failed_round);
}

pub proof fn append_stop(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>, node: NodeId,
    before: Phase, after: Phase, cause: StopCause)
    requires r1 == if !(before is Stopping || before is Closed)
        && (after is Stopping || after is Closed) {
        r0.push(ReportEntry::Stopped { node, cause })
    } else { r0 },
    ensures stop_cause(r0, r1, node, before, after, cause),
{
    reveal(stop_cause);
}

pub proof fn append_stale(r0: Seq<ReportEntry>, r1: Seq<ReportEntry>,
    node: NodeId, reply_kind: ReplyKind)
    requires r1 == r0.push(ReportEntry::Stale { node, reply_kind }),
    ensures stale_refusal(r0, r1, node, reply_kind),
{
    reveal(stale_refusal);
}
}
