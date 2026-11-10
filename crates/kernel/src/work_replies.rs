//! Executable work replies transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
/// Reply rows / #771: one phase change, at most one paid credit and one question.
pub(super) fn reply_plan(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        matches!(reply, Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet),
    ensures reply_family_safety::entry_rows(*old(w), *final(w), i as int, reply),
        (successful_reply_input(old(w).nodes@[i as int], reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint
            || match reply { Reply::PlanLeaf { footprint, .. } => final(w).nodes@[i as int].footprint == footprint, _ => false },
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        reply == Reply::PlanCantMeet && old(w).nodes@[i as int].phase is Planning ==> handler_safety::ladder(old(w).fail_fast,
            true, old(w).nodes@[i as int], final(w).nodes@[i as int], *old(out), *final(out)),
] proof [reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(reply_bounds); reveal(retry_bounds);] {
    kernel_snapshot! { before_plan_node = w.nodes@[i as int] }
    kernel_snapshot! { before_plan_out = *out }
    let c = ReplyCtx { node: i, generation: w.nodes[i].generation,
        reply: reply, fast: w.fail_fast, next_question: w.next_question };
    let change = reply_plan_node(w.nodes[i], c);
    resolve_reply_change(w, i, change, out);
    kernel_proof! {
        if reply == Reply::PlanCantMeet && before_plan_node@.phase is Planning {
            reply_family_safety::plan_retry_ladder(before_plan_node@, c.fast, c.next_question,
                true, change, before_plan_out@, *out, i as int);
        }
    }
    kernel_proof! {
        reveal(retirement::stopped); reveal(handler_safety::ladder);
        reveal(port_t860_contracts::transition_post); reveal(progress_gate_contracts::maker_transition);
    }
}
}

kernel_fn! {
/// Reply rows / #771: one phase change, at most one paid credit and one question.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn reply_work(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox, report: &mut Report) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !matches!(reply, Reply::MakerNeedsFiles { .. } | Reply::IngestDone { .. }
            | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees | Reply::AuditDisagrees
            | Reply::ProtectedPaths | Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet
            | Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
    ensures reply_family_safety::entry_rows(*old(w), *final(w), i as int, reply),
        report::forwards(old(report).entries@, final(report).entries@,
            report::round_entries(NodeId(i as u64), report::work_failure(reply))),
        progress_gate_contracts::maker_transition(*old(w), *final(w), i as int,
            reply, *old(out), *final(out)),
        (successful_reply_input(old(w).nodes@[i as int], reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        handler_safety::fail_fast_reply(reply) && !(old(w).nodes@[i as int].phase is Closed || old(w).nodes@[i as int].phase is Stopping) ==> handler_safety::ladder(old(w).fail_fast,
            handler_safety::work_retry(old(w).nodes@[i as int], reply), old(w).nodes@[i as int], final(w).nodes@[i as int], *old(out), *final(out)),
] proof [reveal(progress_gate_contracts::maker_transition); reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(reply_bounds); reveal(retry_bounds);] {
    let c = ReplyCtx { node: i, generation: w.nodes[i].generation,
        reply: reply, fast: w.fail_fast, next_question: w.next_question };
    let change = reply_work_node(w.nodes[i], c, report);
    resolve_reply_change(w, i, change, out);
    kernel_proof! {
        reveal(retirement::stopped); reveal(handler_safety::ladder);
        reveal(port_t860_contracts::transition_post); reveal(progress_gate_contracts::maker_transition);
    }
}
}

kernel_fn! {
/// Reply rows / #771: one phase change, at most one paid credit and one question.
pub(super) fn reply_rebase(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        matches!(reply, Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges | Reply::RunFailed),
    ensures reply_family_safety::entry_rows(*old(w), *final(w), i as int, reply),
        (successful_reply_input(old(w).nodes@[i as int], reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(reply_bounds); reveal(retry_bounds);] {
    let c = ReplyCtx { node: i, generation: w.nodes[i].generation,
        reply: reply, fast: w.fail_fast, next_question: w.next_question };
    let change = reply_rebase_node(w.nodes[i], c);
    resolve_reply_change(w, i, change, out);
    kernel_proof! {
        reveal(retirement::stopped); reveal(handler_safety::ladder);
        reveal(port_t860_contracts::transition_post); reveal(progress_gate_contracts::maker_transition);
    }
}
}

kernel_fn! {
/// Rebase and outside-push rows / #771: update the observed head before recheck.
/// This changes no counter or field used by the settlement shape projection.
pub(super) fn update_recheck_head(m: &mut Node, head: HeadId) -> (result: ()) [
    ensures *final(m) == (Node {
        head: Some(head), checked: None,
        review_restarted: old(m).head == Some(head) && old(m).review_restarted,
        ..*old(m)
    }),
        final(m).generation == old(m).generation,
        final(m).next_seq == old(m).next_seq,
        final(m).credit == old(m).credit,
        final(m).last_cmd == old(m).last_cmd,
        final(m).phase == old(m).phase, final(m).pos == old(m).pos,
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        final(m).pool == old(m).pool, final(m).holds == old(m).holds,
        settle_shape(*final(m)) == settle_shape(*old(m)),
] {
    if m.head != Some(head) {
        m.review_restarted = false;
    }
    m.head = Some(head);
    m.checked = None;
}
}

kernel_fn! {
/// Reply node work, with completed-stage rows separate from world mutation.
pub(super) fn reply_plan_node(n: Node, c: ReplyCtx) -> (result: ReplyChange) [
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet),
    ensures result == reply_family_values::plan(n, c.fast, c.reply),
        reply_family_values::reply_bound(n, c.next_question, result),
        reply_family_safety::rows(n, c, result),
        (successful_reply_input(n, c.reply) ==> distance(reply_family_values::finish(c.next_question, result)) < distance(n)),
        (c.reply == Reply::PlanCantMeet && c.fast && n.phase is Planning ==>
            reply_family_values::finish(c.next_question, result).phase == Phase::Stopping(Outcome::Stopped)
                && reply_family_values::finish(c.next_question, result).review_stop == Some(ReviewStop::PlanCantMeet)),
] proof [reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(retirement::stopped);] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    let i = c.node;
    let reply = c.reply;
match reply {
        Reply::PlanLeaf { footprint, fits } => {
            let m = &mut node;
            let must_split = matches!(m.phase, Phase::Planning { must_split: true });
            if fits && !footprint.is_empty() && !must_split {
                m.footprint = footprint;
                m.attempt = 1;
                m.review_restarted = false;
                m.round = 0;
                m.plan_retries = 0;
                set_phase(m, Phase::Ready(Resume::Making));
            } else {
                { let value = plan_retry_node(node, c.fast, c.next_question, false); kernel_proof! { reply_family_safety::plan_rows(n, c, value); } return value; }
            }
        }
        Reply::PlanSplit => { let value = ReplyChange::Ask { node, kind: QKind::NeedsSplitting, cont: Cont::Nowhere, cost: 0 }; kernel_proof! { reply_family_safety::plan_rows(n, c, value); } return value; },
        Reply::PlanCantMeet => { let value = plan_retry_node(node, c.fast, c.next_question, true); kernel_proof! { reply_family_safety::plan_rows(n, c, value); } return value; },
        _ => {}
    }
    { let value = ReplyChange::Done { node, note }; kernel_proof! { reply_family_safety::plan_rows(n, c, value); } return value; }
}
}

kernel_fn! {
/// Reply node work, with completed-stage rows separate from world mutation.
pub(super) fn reply_work_node(n: Node, c: ReplyCtx, report: &mut Report) -> (result: ReplyChange) [
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        !matches!(c.reply, Reply::MakerNeedsFiles { .. } | Reply::IngestDone { .. }
            | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees | Reply::AuditDisagrees
            | Reply::ProtectedPaths | Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet
            | Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
    ensures result == reply_family_values::work(n, c.fast, c.reply),
        reply_family_values::reply_bound(n, c.next_question, result),
        reply_family_safety::rows(n, c, result),
        (successful_reply_input(n, c.reply) ==> distance(reply_family_values::finish(c.next_question, result)) < distance(n)),
        reply_family_values::finish(c.next_question, result).footprint == n.footprint,
        report::forwards(old(report).entries@, final(report).entries@,
            report::round_entries(NodeId(c.node as u64), report::work_failure(c.reply))),
] proof [reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(retirement::stopped);] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    let i = c.node;
    let reply = c.reply;
    kernel_snapshot! { before_report = report.entries@ }
match reply {
        Reply::MakerDone { head } => {
            let m = &mut node;
            m.head = Some(head);
            m.checked = None;
            set_phase(m, Phase::Checking);
        }
        Reply::MakerCantMeet => {
            { let value = next_rung_node(node, i, c.fast, c.next_question, ReviewStop::MakerCantMeet, report); kernel_proof! { report::lemmas::work_kind(reply);
            reveal(report::forwards); reveal(report::round_entries); reveal(report::failed_round); reveal(report::failure_cause);
            report::lemmas::forward_round(before_report@, report.entries@, NodeId(i as u64), report::work_failure(reply));
            reply_family_safety::work_rows(n, c, value); } return value; }

        }
        Reply::MakerOutOfRoom => { let value = out_of_room_node(node, c.fast, c.next_question, ReviewStop::MakerOutOfRoom); kernel_proof! { report::lemmas::work_kind(reply);
            reveal(report::forwards); reveal(report::round_entries); reveal(report::failed_round); reveal(report::failure_cause);
            report::lemmas::forward_round(before_report@, report.entries@, NodeId(i as u64), report::work_failure(reply));
            reply_family_safety::work_rows(n, c, value); } return value; },
        Reply::ChecksDone {
            pass, in_footprint, ..
        } => {
            if pass && in_footprint {
                let m = &mut node;
                m.checked = m.head;
                set_phase(m, Phase::Reviewing);

            } else {
                { let value = failed_round_node(node, i, c.fast, c.next_question, ReviewStop::ChecksFailed, report); kernel_proof! { report::lemmas::work_kind(reply);
            reveal(report::forwards); reveal(report::round_entries); reveal(report::failed_round); reveal(report::failure_cause);
            report::lemmas::forward_round(before_report@, report.entries@, NodeId(i as u64), report::work_failure(reply));
            reply_family_safety::work_rows(n, c, value); } return value; }

            }
        }
        Reply::Approved { head } => {
            let m = &mut node;
            m.approved = Some(head);
            set_phase(m, Phase::Publishing);
        }
        Reply::ChangesRequested => {
            { let value = failed_round_node(node, i, c.fast, c.next_question, ReviewStop::ChangesRequested, report); kernel_proof! { report::lemmas::work_kind(reply);
            reveal(report::forwards); reveal(report::round_entries); reveal(report::failed_round); reveal(report::failure_cause);
            report::lemmas::forward_round(before_report@, report.entries@, NodeId(i as u64), report::work_failure(reply));
            reply_family_safety::work_rows(n, c, value); } return value; }

        }
        Reply::ReviewerOutOfRoom => {
            if node.review_restarted {
                { let value = out_of_room_node(node, c.fast, c.next_question, ReviewStop::ReviewerOutOfRoom); kernel_proof! { report::lemmas::work_kind(reply);
            reveal(report::forwards); reveal(report::round_entries); reveal(report::failed_round); reveal(report::failure_cause);
            report::lemmas::forward_round(before_report@, report.entries@, NodeId(i as u64), report::work_failure(reply));
            reply_family_safety::work_rows(n, c, value); } return value; }
            } else {
                node.review_restarted = true;

                { let value = recheck_node(node, c.next_question, Phase::Reviewing); kernel_proof! { report::lemmas::work_kind(reply);
            reveal(report::forwards); reveal(report::round_entries); reveal(report::failed_round); reveal(report::failure_cause);
            report::lemmas::forward_round(before_report@, report.entries@, NodeId(i as u64), report::work_failure(reply));
            reply_family_safety::work_rows(n, c, value); } return value; }

            }
        }
        _ => { let value = reply_rebase_node(node, c); kernel_proof! { report::lemmas::work_kind(reply);
            reveal(report::forwards); reveal(report::round_entries); reveal(report::failed_round); reveal(report::failure_cause);
            report::lemmas::forward_round(before_report@, report.entries@, NodeId(i as u64), report::work_failure(reply));
            reply_family_safety::work_rows(n, c, value); } return value; },
    }
    { let value = ReplyChange::Done { node, note }; kernel_proof! { report::lemmas::work_kind(reply);
            reveal(report::forwards); reveal(report::round_entries); reveal(report::failed_round); reveal(report::failure_cause);
            report::lemmas::forward_round(before_report@, report.entries@, NodeId(i as u64), report::work_failure(reply));
            reply_family_safety::work_rows(n, c, value); } return value; }
}
}

kernel_fn! {
/// Reply node work, with completed-stage rows separate from world mutation.
pub(super) fn reply_rebase_node(n: Node, c: ReplyCtx) -> (result: ReplyChange) [
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges | Reply::RunFailed),
    ensures result == reply_family_values::rebase(n, c.reply),
        reply_family_values::reply_bound(n, c.next_question, result),
        reply_family_safety::rows(n, c, result),
        (successful_reply_input(n, c.reply) ==> distance(reply_family_values::finish(c.next_question, result)) < distance(n)),
        reply_family_values::finish(c.next_question, result).footprint == n.footprint,
] proof [reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(retirement::stopped);] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    let i = c.node;
    let reply = c.reply;
match reply {
        Reply::RebaseClean { head } => {
            update_recheck_head(&mut node, head);
            { let value = recheck_node(node, c.next_question, Phase::Checking); kernel_proof! { reply_family_safety::work_rows(n, c, value); } return value; }
        }
        Reply::RebaseNeedsChanges => {
            let m = &mut node;
            if m.credit.can_spend(1) {
                m.credit.spend(1);
                set_phase(m, Phase::Making);
            } else {
                { let value = ReplyChange::Ask { node, kind: QKind::OutOfCredit, cont: Cont::Ready(Resume::Making), cost: 1, }; kernel_proof! { reply_family_safety::work_rows(n, c, value); } return value; }
            }
        }
        _ => {}
    }
    { let value = ReplyChange::Done { node, note }; kernel_proof! { reply_family_safety::work_rows(n, c, value); } return value; }
}
}
