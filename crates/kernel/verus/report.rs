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

pub(in super::super) proof fn entries_for_event(event: Event, input: ReplyInput)
    ensures event_entries(event, input) == match event {
        Event::Reply { node, generation, reply } => reply_entries(node, input.phase,
            input.current, input.run, input.job, generation, reply, input.accepted),
        _ => Seq::empty(),
    },
{ reveal(event_entries); }

pub(in super::super) proof fn observe(event: Event, before: Seq<Phase>, after: Seq<Phase>,
    input: ReplyInput, review: Option<ReviewStop>, r0: Seq<ReportEntry>, middle: Seq<ReportEntry>,
    r1: Seq<ReportEntry>)
    requires event is Fact ==> before.len() == after.len(),
        before.len() <= after.len() <= before.len() + 1
        && (forall|j: int| 0 <= j < before.len() && event_target(event) != Some(NodeId(j as u64))
            ==> terminal_phase_frame(before[j], after[j]))
        && (forall|j: int| before.len() <= j < after.len() ==> !(after[j] is Stopping || after[j] is Closed))
        && (!(event is Fact) ==> match event_target(event) {
            Some(node) if node.0 < before.len() =>
                forwards(r0, middle, event_entries(event, input))
                && stop_cause(middle, r1, node, before[node.0 as int], after[node.0 as int],
                    stop_kind(after[node.0 as int], review, false, user_stop(event))),
            _ => forwards(r0, r1, Seq::empty()),
        }),
    ensures agrees_except_fact_stops_and_ci_failed(event, before, after, input, r0, r1),
{
    reveal(agrees_except_fact_stops_and_ci_failed);
    if event is Fact {
        assert forall|j: int| 0 <= j < after.len()
            implies terminal_phase_frame(after[j], after[j]) by { phase_refl(after[j]); }
        settle_result(after, after, r1, r1);
        assert(before.len() == after.len());
        let boundary = (after, r1);
        assert(boundary.0.len() == before.len()
            && settled_forwards(boundary.0, after, boundary.1, r1));
        assert(exists|fact_result: (Seq<Phase>, Seq<ReportEntry>)|
            fact_result.0.len() == before.len()
            && settled_forwards(fact_result.0, after, fact_result.1, r1));
    } else {
        if let Some(node) = event_target(event) {
            if node.0 < before.len() {
                phase_refl(after[node.0 as int]);
                assert(stop_trace((after[node.0 as int], review, middle), event, node,
                    before[node.0 as int], after[node.0 as int], input, r0, r1));
            }
        }
    }
}

pub(in super::super) proof fn through_settle(event: Event, before: Seq<Phase>, applied: Seq<Phase>,
    after: Seq<Phase>, input: ReplyInput, r0: Seq<ReportEntry>, r1: Seq<ReportEntry>, r2: Seq<ReportEntry>)
    requires agrees_except_fact_stops_and_ci_failed(event, before, applied, input, r0, r1),
        settled_forwards(applied, after, r1, r2),
    ensures agrees_except_fact_stops_and_ci_failed(event, before, after, input, r0, r2),
{
    reveal(agrees_except_fact_stops_and_ci_failed);
    reveal(settled_forwards);
    read_forwards(r1, r2, Seq::empty());
    assert(r1 + Seq::empty() =~= r1);
    {
        assert forall|j: int| 0 <= j < before.len()
            && event_target(event) != Some(NodeId(j as u64))
            implies terminal_phase_frame(before[j], after[j]) by {
            phase_trans(before[j], applied[j], after[j]);
        }
        assert forall|j: int| before.len() <= j < after.len()
            implies !(after[j] is Stopping || after[j] is Closed) by {
            reveal(terminal_phase_frame);
        }
        if event is Fact {
            let fact_result = choose|fact_result: (Seq<Phase>, Seq<ReportEntry>)|
                fact_result.0.len() == before.len()
                && settled_forwards(fact_result.0, applied, fact_result.1, r1);
            reveal(settled_forwards);
            read_forwards(fact_result.1, r1, Seq::empty());
            assert(fact_result.1 + Seq::empty() =~= fact_result.1);
            assert forall|j: int| 0 <= j < fact_result.0.len()
                implies terminal_phase_frame(fact_result.0[j], after[j]) by {
                phase_trans(fact_result.0[j], applied[j], after[j]);
            }
            settle_result(fact_result.0, after, fact_result.1, r2);
            assert(fact_result.0.len() == before.len()
                && settled_forwards(fact_result.0, after, fact_result.1, r2));
            assert(exists|boundary: (Seq<Phase>, Seq<ReportEntry>)|
                boundary.0.len() == before.len()
                && settled_forwards(boundary.0, after, boundary.1, r2));
        } else if let Some(node) = event_target(event) {
            if node.0 < before.len() {
                let witness = choose|witness: (Phase, Option<ReviewStop>, Seq<ReportEntry>)|
                    stop_trace(witness, event, node, before[node.0 as int], applied[node.0 as int], input, r0, r1);
                phase_trans(witness.0, applied[node.0 as int], after[node.0 as int]);
                assert(stop_trace(witness, event, node, before[node.0 as int], after[node.0 as int], input, r0, r2));
            }
        }
    }
}

}
