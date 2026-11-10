//! Executable reply admission transitions and their #771 safety contracts.
use super::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Copy, Debug, PartialEq, Eq)]
pub(super) enum ReplyChange {
    Done { node: Node, note: Option<Notice> },
    Ask { node: Node, kind: QKind, cont: Cont, cost: u8 },
    Incident { node: Node, needs_rob: bool },
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Copy)]
pub(super) struct ReplyCtx {
    pub node: usize,
    pub generation: u64,
    pub reply: Reply,
    pub fast: bool,
    pub next_question: u64,
}

#[cfg(verus_keep_ghost)]
vstd::prelude::verus! {
impl Clone for ReplyChange {
    fn clone(&self) -> (result: Self)
        ensures result == *self,
    {
        *self
    }
}
}
#[cfg(not(verus_keep_ghost))]
impl Clone for ReplyChange {
    fn clone(&self) -> Self { *self }
}

#[cfg(verus_keep_ghost)]
vstd::prelude::verus! {
impl Clone for ReplyCtx {
    fn clone(&self) -> (result: Self)
        ensures result == *self,
    {
        *self
    }
}
}
#[cfg(not(verus_keep_ghost))]
impl Clone for ReplyCtx {
    fn clone(&self) -> Self { *self }
}

kernel_fn! {
/// Reply rows / #771: phase, schema and commit identity use the executable checks.
pub(super) fn accepts(w: &World, n: &Node, reply: &Reply) -> (result: bool) [
    ensures result == port_t860_contracts::admissible(*w, *n, *reply),
        result == match (n.phase, *reply) {
        (Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }, Reply::ProtectedPaths) => true,
        (Phase::Ingesting, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)) => true,
        (Phase::Auditing, Reply::AuditAgrees | Reply::AuditDisagrees) => true,
        (Phase::Planning { .. }, Reply::PlanLeaf { footprint, .. }) =>
            footprint::valid_footprint(*w, footprint),
        (Phase::Planning { .. }, Reply::PlanSplit | Reply::PlanCantMeet
            | Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit) => true,
        (Phase::Making, Reply::MakerDone { .. }) => true,
        (Phase::Making, Reply::MakerNeedsFiles { files, .. }) =>
            footprint::valid_footprint(*w, files),
        (Phase::Making, Reply::MakerCantMeet | Reply::MakerOutOfRoom) => true,
        (Phase::Checking, Reply::ChecksDone { report, .. }) => report,
        (Phase::Reviewing, Reply::Approved { head }) =>
            n.head == Some(head) && n.checked == Some(head),
        (Phase::Reviewing, Reply::ChangesRequested | Reply::ReviewerOutOfRoom) => true,
        (Phase::Rebasing, Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges) => true,
        _ => false,
    },
] proof [reveal(port_t860_contracts::admissible);] {
    match (n.phase, reply) {
        (Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }, Reply::ProtectedPaths) => {
            true
        }
        (Phase::Ingesting, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)) => true,
        (Phase::Auditing, Reply::AuditAgrees | Reply::AuditDisagrees) => true,
        (Phase::Planning { .. }, Reply::PlanLeaf { footprint, .. }) => w.has_footprint(*footprint),
        (Phase::Planning { .. }, Reply::PlanSplit | Reply::PlanCantMeet
            | Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit) => true,
        (Phase::Making, Reply::MakerDone { .. }) => true,
        (Phase::Making, Reply::MakerNeedsFiles { files, .. }) => w.has_footprint(*files),
        (Phase::Making, Reply::MakerCantMeet | Reply::MakerOutOfRoom) => true,
        // No report means the tests never ran: an infrastructure failure.
        (Phase::Checking, Reply::ChecksDone { report, .. }) => *report,
        (Phase::Reviewing, Reply::Approved { head }) => {
            n.head == Some(*head) && n.checked == Some(*head)
        }
        (Phase::Reviewing, Reply::ChangesRequested | Reply::ReviewerOutOfRoom) => true,
        (Phase::Rebasing, Reply::RebaseClean { .. }) => true,
        (Phase::Rebasing, Reply::RebaseNeedsChanges) => true,
        _ => false,
    }
}
}

kernel_fn! {
/// Reply rows / #771: stale operations stutter, invalid replies use the bounded
/// failure handler, and successful replies use the actual transition bodies.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn on_reply(w: &mut World, i: usize, generation: u64, reply: Reply,
    out: &mut Outbox, report: &mut Report) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
        footprint::valid_footprint(*old(w), old(w).nodes@[i as int].footprint),
        footprint::ordered_table(*old(w)),
    ensures report::forwards(old(report).entries@, final(report).entries@,
            report::reply_entries(NodeId(i as u64), old(w).nodes@[i as int].phase, current_item(old(w).nodes@[i as int]), old(w).nodes@[i as int].run, old(w).nodes@[i as int].job, generation, reply, port_t860_contracts::admissible(*old(w), old(w).nodes@[i as int], reply))),
        port_t860_contracts::reply_post(*old(w), *final(w), i as int, generation, reply, *old(out), *final(out)),
        progress_gate_contracts::maker_reply(*old(w), *final(w), i as int,
            generation, reply, *old(out), *final(out)),
        (node_checked(*old(w), old(w).nodes@[i as int])
            && generation == old(w).nodes@[i as int].generation
            && successful_reply_input(old(w).nodes@[i as int], reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        footprint::valid_footprint(*final(w), final(w).nodes@[i as int].footprint),
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start as int <= old(w).nodes@[i as int].credit.start as int + CREDIT_START_STEP_BUDGET,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        footprint::ordered_table(*final(w)),
        final(w).file_sets@.len() >= old(w).file_sets@.len(),
        forall|j: int| 0 <= j < old(w).file_sets@.len()
            ==> final(w).file_sets@[j] == old(w).file_sets@[j],
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        (matches!(reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit)
            && old(w).nodes@[i as int].phase is Planning
            && current_item(old(w).nodes@[i as int]) == Some(Item::Await)
            && old(w).nodes@[i as int].run == Some(generation) ==> {
                let n = final(w).nodes@[i as int];
                n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
                    && n.review_stop == Some(ReviewStop::PlanCantMeet)
                    && n.credit == old(w).nodes@[i as int].credit
                    && n.plan_retries == old(w).nodes@[i as int].plan_retries
                    && final(w).next_question == old(w).next_question
                    && *final(out) == *old(out)
            }),
        (reply == Reply::AuditDisagrees && old(w).nodes@[i as int].phase == Phase::Auditing
            && old(w).nodes@[i as int].audit_rounds >= 1
            && current_item(old(w).nodes@[i as int]) == Some(Item::Await)
            && old(w).nodes@[i as int].run == Some(generation) ==> {
                let n = final(w).nodes@[i as int];
                n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
                    && n.review_stop == Some(ReviewStop::AuditDisagreement)
                    && n.credit == old(w).nodes@[i as int].credit
                    && final(w).next_question == old(w).next_question
                    && *final(out) == *old(out)
            }),
        transition_safety::fail_fast_event(Event::Reply { node: NodeId(i as u64), generation, reply }) ==> handler_safety::reply_rows(old(w).fail_fast,
            transition_safety::retry_due(*old(w), Event::Reply { node: NodeId(i as u64), generation, reply }), transition_safety::funding_due(*old(w), Event::Reply { node: NodeId(i as u64), generation, reply }),
            old(w).nodes@[i as int], final(w).nodes@[i as int], *old(out), *final(out)),
] {
    kernel_snapshot! { before_report = report.entries@ }
    kernel_snapshot! { before_reply = *w }
    kernel_snapshot! { before_outputs = *out }
    let n = &w.nodes[i];
    if n.phase.is_stopping() {
        on_stopping_reply(w, i, generation, reply, out);
        kernel_proof! { handler_safety::quiet_due(before_reply@, w.nodes@[i as int], i, generation, reply, before_outputs@, *out); }
        kernel_proof! { report::lemmas::reply_result(before_report@, report.entries@, NodeId(i as u64),
            before_reply@.nodes@[i as int].phase, current_item(before_reply@.nodes@[i as int]),
            before_reply@.nodes@[i as int].run, before_reply@.nodes@[i as int].job,
            generation, reply, port_t860_contracts::admissible(before_reply@, before_reply@.nodes@[i as int], reply)); }
        return;
    }
    if n.current() != Some(Item::Await) || (n.run != Some(generation) && n.job != Some(generation))
    {
        report.record_stale(NodeId(i as u64), ReplyKind::of(reply));
        kernel_proof! { report::lemmas::read_stale(before_report@, report.entries@,
            NodeId(i as u64), report::reply_kind(reply)); }
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        kernel_proof! {
            progress_gate_contracts::table_order_frame(before_reply@, *w);
            progress_gate_contracts::maker_reply_inactive(before_reply@, *w, i as int,
                generation, reply, before_outputs@, *out);
        }
        kernel_proof! {
            port_t860_link::inactive(before_reply@, *w, i as int, generation, reply, before_outputs@, *out);
        }
        kernel_proof! { report::lemmas::reply_result(before_report@, report.entries@, NodeId(i as u64),
            before_reply@.nodes@[i as int].phase, current_item(before_reply@.nodes@[i as int]),
            before_reply@.nodes@[i as int].run, before_reply@.nodes@[i as int].job,
            generation, reply, port_t860_contracts::admissible(before_reply@, before_reply@.nodes@[i as int], reply)); }
kernel_proof! { handler_safety::quiet_due(before_reply@, w.nodes@[i as int], i, generation, reply, before_outputs@, *out); }
        return; // stale
    }
    // A reply that breaks its schema or names the wrong commit counts as a failed run.
    kernel_proof! {
        if successful_reply_input(*n, reply) {
            if let Reply::PlanLeaf { footprint, .. } = reply {
                if footprint::footprint_empty(n.footprint) { footprint::inline_is_valid(*w, 1); }
            }
        }
    }
    if !accepts(w, n, &reply) {
        on_rejected_reply(w, i, generation, reply, out, report);
        kernel_proof! { handler_safety::quiet_due(before_reply@, w.nodes@[i as int], i, generation, reply, before_outputs@, *out); }
        kernel_proof! { report::lemmas::round_delta(before_report@, report.entries@,
            NodeId(i as u64), Some(FailedRoundCause::RunFailed)); }
        kernel_proof! { report::lemmas::reply_result(before_report@, report.entries@, NodeId(i as u64),
            before_reply@.nodes@[i as int].phase, current_item(before_reply@.nodes@[i as int]),
            before_reply@.nodes@[i as int].run, before_reply@.nodes@[i as int].job,
            generation, reply, port_t860_contracts::admissible(before_reply@, before_reply@.nodes@[i as int], reply)); }
        return;
    }
    kernel_proof! {
        if reply == Reply::AuditDisagrees {
            assert(w.nodes@[i as int].phase == Phase::Auditing);
        }
        reply_family_safety::admitted_stage(before_reply@,
            port_t860_contracts::ended(before_reply@.nodes@[i as int]),
            ReplyCtx { node: i, generation, reply, fast: before_reply@.fail_fast,
                next_question: before_reply@.next_question });
    }
    on_admitted_reply(w, i, generation, reply, out, report);
    kernel_proof! {
        if handler_safety::fail_fast_reply(reply) {
            handler_safety::admitted_due(before_reply@, w.nodes@[i as int], i, generation, reply,
                before_outputs@, *out);
        }
    }
    kernel_proof! {
        report::lemmas::read_forwards(before_report@, report.entries@,
            report::round_entries(NodeId(i as u64), report::work_failure(reply)));
        report::lemmas::reply_result(before_report@, report.entries@, NodeId(i as u64),
            before_reply@.nodes@[i as int].phase, current_item(before_reply@.nodes@[i as int]),
            before_reply@.nodes@[i as int].run, before_reply@.nodes@[i as int].job,
            generation, reply, port_t860_contracts::admissible(before_reply@, before_reply@.nodes@[i as int], reply)); }

}
}

kernel_fn! {
/// Reply admission / #860: verbatim admitted block, isolated under the standing refactor rule.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn on_admitted_reply(w: &mut World, i: usize, _generation: u64, reply: Reply,
    out: &mut Outbox, report: &mut Report) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
        footprint::valid_footprint(*old(w), old(w).nodes@[i as int].footprint),
        footprint::ordered_table(*old(w)),
        !(old(w).nodes@[i as int].phase is Stopping || old(w).nodes@[i as int].phase is Closed),
        current_item(old(w).nodes@[i as int]) == Some(Item::Await),
        old(w).nodes@[i as int].run == Some(_generation) || old(w).nodes@[i as int].job == Some(_generation),
        (reply == Reply::AuditDisagrees ==> old(w).nodes@[i as int].phase == Phase::Auditing),
        (match reply {
            Reply::PlanLeaf { footprint, .. } | Reply::MakerNeedsFiles { files: footprint, .. } =>
                footprint::valid_footprint(*old(w), footprint),
            _ => true,
        }),
    ensures report::forwards(old(report).entries@, final(report).entries@,
            report::round_entries(NodeId(i as u64), report::work_failure(reply))),
        port_t860_contracts::reply_post(*old(w), *final(w), i as int, _generation, reply, *old(out), *final(out)),
        progress_gate_contracts::maker_reply(*old(w), *final(w), i as int,
            _generation, reply, *old(out), *final(out)),
        (node_checked(*old(w), old(w).nodes@[i as int])
            && _generation == old(w).nodes@[i as int].generation
            && successful_reply_input(old(w).nodes@[i as int], reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        footprint::valid_footprint(*final(w), final(w).nodes@[i as int].footprint),
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start as int <= old(w).nodes@[i as int].credit.start as int + CREDIT_START_STEP_BUDGET,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        footprint::ordered_table(*final(w)),
        final(w).file_sets@.len() >= old(w).file_sets@.len(),
        forall|j: int| 0 <= j < old(w).file_sets@.len()
            ==> final(w).file_sets@[j] == old(w).file_sets@[j],
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        (matches!(reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit)
            && old(w).nodes@[i as int].phase is Planning
            && current_item(old(w).nodes@[i as int]) == Some(Item::Await)
            && old(w).nodes@[i as int].run == Some(_generation) ==> {
                let n = final(w).nodes@[i as int];
                n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
                    && n.review_stop == Some(ReviewStop::PlanCantMeet)
                    && n.credit == old(w).nodes@[i as int].credit
                    && n.plan_retries == old(w).nodes@[i as int].plan_retries
                    && final(w).next_question == old(w).next_question
                    && *final(out) == *old(out)
            }),
        (reply == Reply::AuditDisagrees && old(w).nodes@[i as int].phase == Phase::Auditing
            && old(w).nodes@[i as int].audit_rounds >= 1
            && current_item(old(w).nodes@[i as int]) == Some(Item::Await)
            && old(w).nodes@[i as int].run == Some(_generation) ==> {
                let n = final(w).nodes@[i as int];
                n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
                    && n.review_stop == Some(ReviewStop::AuditDisagreement)
                    && n.credit == old(w).nodes@[i as int].credit
                    && final(w).next_question == old(w).next_question
                    && *final(out) == *old(out)
            }),
        handler_safety::fail_fast_admitted(old(w).nodes@[i as int], reply) ==> handler_safety::reply_rows(old(w).fail_fast,
            handler_safety::admitted_retry(old(w).nodes@[i as int], reply), handler_safety::admitted_fund(old(w).nodes@[i as int], reply),
            old(w).nodes@[i as int], final(w).nodes@[i as int], *old(out), *final(out)),
] {
    kernel_snapshot! { before_report = report.entries@ }
    kernel_proof! { report::lemmas::work_kind(reply); }
    kernel_snapshot! { before_reply = *w }
    kernel_snapshot! { before_outputs = *out }
    kernel_snapshot! { before_operation = *w }
    w.nodes[i] = reply_end_node(w.nodes[i]);
    kernel_proof! {
        if port_t860_contracts::admissible(before_operation@, before_operation@.nodes@[i as int], reply) {
            reply_family_safety::admitted_stage(before_operation@, w.nodes@[i as int],
                ReplyCtx { node: i, generation: _generation, reply,
                    fast: before_operation@.fail_fast, next_question: before_operation@.next_question });
        }
    }
    kernel_proof! {
        footprint::valid_under_table_growth(before_operation@, *w, w.nodes@[i as int].footprint);
    }
    kernel_snapshot! { after_operation = *w }
    kernel_proof! {
        progress_gate_contracts::table_order_frame(before_reply@, *w);
        if successful_reply_input(before_reply@.nodes@[i as int], reply) {
            progress_gate_contracts::end_reply_progress(before_reply@.nodes@[i as int],
                w.nodes@[i as int], reply);
        }
    }
    match reply {
        Reply::MakerNeedsFiles { files, fits } => {
            kernel_proof! { footprint::valid_under_table_growth(before_operation@, *w, files); }
            reply_files(w, i, files, fits, out);
            kernel_proof! {
                if handler_safety::fail_fast_admitted(before_reply@.nodes@[i as int], reply) {
                    handler_safety::files_admitted(before_reply@.fail_fast, files, fits,
                        before_reply@.nodes@[i as int], after_operation@.nodes@[i as int],
                        w.nodes@[i as int], before_outputs@, *out);
                }
                port_t860_link::irrelevant(before_reply@, *w, i as int, _generation, reply, before_outputs@, *out);
                progress_gate_contracts::maker_reply_inactive(before_reply@, *w, i as int,
                    _generation, reply, before_outputs@, *out);
            }
        }
        _ => {
            reply_transition(w, i, reply, out, report);
            kernel_proof! {
                if handler_safety::fail_fast_admitted(before_reply@.nodes@[i as int], reply) {
                    handler_safety::transition_admitted(before_reply@.fail_fast, reply,
                        before_reply@.nodes@[i as int], after_operation@.nodes@[i as int],
                        w.nodes@[i as int], before_outputs@, *out);
                }
            }
            kernel_proof! { report::lemmas::round_delta(before_report@, report.entries@,
                NodeId(i as u64), report::work_failure(reply)); }
            kernel_proof! {
                assert(port_t860_contracts::reply_post(before_reply@, *w, i as int, _generation, reply, before_outputs@, *out)) by {
                    if reply == Reply::AuditDisagrees {
                        port_t860_link::admitted(before_reply@, after_operation@, *w, i as int, _generation, reply, before_outputs@, *out);
                    } else {
                        port_t860_link::irrelevant(before_reply@, *w, i as int, _generation, reply, before_outputs@, *out);
                    }
                }
                progress_gate_contracts::reply_bounds_result(after_operation@, *w, i as int,
                    before_outputs@.len, out.len);
                progress_gate_contracts::table_order_frame(after_operation@, *w);
                progress_gate_contracts::maker_reply_composed(before_reply@, after_operation@,
                    *w, i as int, _generation, reply, before_outputs@, *out);
                if let Reply::PlanLeaf { footprint, .. } = reply {
                    footprint::valid_under_table_growth(before_operation@, *w, footprint);
                }
                footprint::valid_under_table_growth(before_operation@, *w, before_operation@.nodes@[i as int].footprint);
            }
        }
    }
    kernel_proof! { report::lemmas::forward_round(before_report@, report.entries@,
        NodeId(i as u64), report::work_failure(reply)); }

}
}

kernel_fn! {
/// File extension rows / #771: union only appends file storage; the transition
/// spends at most one credit and allocates one generation or question.
pub(super) fn reply_files(w: &mut World, i: usize, files: Footprint, fits: bool,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
        footprint::valid_footprint(*old(w), old(w).nodes@[i as int].footprint),
        footprint::valid_footprint(*old(w), files), footprint::ordered_table(*old(w)),
    ensures reply_family_safety::entry_rows(*old(w), *final(w), i as int, Reply::MakerNeedsFiles { files, fits }),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        footprint::valid_footprint(*final(w), final(w).nodes@[i as int].footprint),
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        footprint::ordered_table(*final(w)),
        final(w).file_sets@.len() >= old(w).file_sets@.len(),
        forall|j: int| 0 <= j < old(w).file_sets@.len()
            ==> final(w).file_sets@[j] == old(w).file_sets@[j],
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        !(old(w).nodes@[i as int].phase is Closed || old(w).nodes@[i as int].phase is Stopping) ==> handler_safety::files_post(old(w).fail_fast, files, fits, old(w).nodes@[i as int], final(w).nodes@[i as int], *old(out), *final(out)),
] proof [reveal(footprint::ordered_table);] {
    kernel_snapshot! { before_files = *w }
    kernel_snapshot! { before_files_out = *out }
    if fits && !files.is_empty() && w.nodes[i].credit.can_spend(1) {
        let footprint = w.union_footprints(w.nodes[i].footprint, files);
        kernel_snapshot! { after_union = *w }
        let c = ReplyCtx { node: i, generation: w.nodes[i].generation,
            reply: Reply::MakerNeedsFiles { files, fits }, fast: w.fail_fast,
            next_question: w.next_question };
        let change = reply_files_paid(w.nodes[i], footprint);
        kernel_proof! {
            reply_family_posts::files_arms(before_files@.nodes@[i as int], c.fast, files, fits, footprint);
            reply_family_safety::files_rows(before_files@.nodes@[i as int], c,
            files, fits, footprint, change); }
        resolve_reply_change(w, i, change, out);
        kernel_proof! {
            reply_family_posts::files_posts(before_files@.nodes@[i as int], before_files@.fail_fast,
                before_files@.next_question, files, fits, footprint, i as int, before_files_out@, *out);
            handler_safety::files_rows(before_files@.fail_fast, files, fits, before_files@.nodes@[i as int],
                w.nodes@[i as int], before_files_out@, *out);
        }
        kernel_proof! { footprint::valid_under_table_growth(after_union@, *w, footprint); }
    } else if fits && !files.is_empty() && w.fail_fast {
        // Q54 stops retries, not credit decisions: Rob funds the extension.
        let footprint = w.union_footprints(w.nodes[i].footprint, files);
        kernel_snapshot! { after_union = *w }
        let c = ReplyCtx { node: i, generation: w.nodes[i].generation,
            reply: Reply::MakerNeedsFiles { files, fits }, fast: w.fail_fast,
            next_question: w.next_question };
        let change = reply_files_ask(w.nodes[i], footprint);
        kernel_proof! {
            reply_family_posts::files_arms(before_files@.nodes@[i as int], c.fast, files, fits, footprint);
            reply_family_safety::files_rows(before_files@.nodes@[i as int], c,
            files, fits, footprint, change); }
        resolve_reply_change(w, i, change, out);
        kernel_proof! {
            reply_family_posts::files_posts(before_files@.nodes@[i as int], before_files@.fail_fast,
                before_files@.next_question, files, fits, footprint, i as int, before_files_out@, *out);
            handler_safety::fund_fast(before_files@.nodes@[i as int], w.nodes@[i as int]);
            handler_safety::files_rows(before_files@.fail_fast, files, fits, before_files@.nodes@[i as int],
                w.nodes@[i as int], before_files_out@, *out);
        }
        kernel_proof! { footprint::valid_under_table_growth(after_union@, *w, footprint); }
    } else {
        let c = ReplyCtx { node: i, generation: w.nodes[i].generation,
            reply: Reply::MakerNeedsFiles { files, fits }, fast: w.fail_fast,
            next_question: w.next_question };
        let change = out_of_room_node(w.nodes[i], w.fail_fast, w.next_question, ReviewStop::MakerNeedsFiles);
        kernel_proof! {
            reveal(reply_family_values::files);
            reply_family_safety::files_rows(before_files@.nodes@[i as int], c,
                files, fits, before_files@.nodes@[i as int].footprint, change);
        }
        resolve_reply_change(w, i, change, out);
        kernel_proof! {
            reply_family_posts::room_posts(before_files@.nodes@[i as int], before_files@.fail_fast,
                before_files@.next_question, ReviewStop::MakerNeedsFiles, i as int, before_files_out@, *out);
            if !(before_files@.nodes@[i as int].phase is Closed || before_files@.nodes@[i as int].phase is Stopping) {
                if fits && !footprint::footprint_empty(files) {
                    handler_safety::fund_off(before_files@.nodes@[i as int], w.nodes@[i as int]);
                }
                handler_safety::files_rows(before_files@.fail_fast, files, fits, before_files@.nodes@[i as int],
                    w.nodes@[i as int], before_files_out@, *out);
            }
        }
        kernel_proof! { footprint::valid_under_table_growth(before_files@, *w, w.nodes@[i as int].footprint); }
    }
    }
}

kernel_fn! {
/// The two fitting-extension paths, after the original conditional union call.
/// funded is the branch tag already selected by reply_files, not a new guard.
pub(super) fn reply_files_node(n: Node, fast: bool, files: Footprint, fits: bool,
    union: Footprint, funded: bool) -> (result: ReplyChange) [
    requires node_headroom(n), arithmetic_consistent(n),
        fits, !footprint::footprint_empty(files),
        funded == transition_safety::can_spend_one(n), funded || fast,
    ensures result == reply_family_values::files(n, fast, files, fits, union),
        reply_family_values::callable(result),
] {
    if funded {
        let result = reply_files_paid(n, union);
        kernel_proof! { reply_family_posts::files_arms(n, fast, files, fits, union); }
        result
    } else {
        let result = reply_files_ask(n, union);
        kernel_proof! { reply_family_posts::files_arms(n, fast, files, fits, union); }
        result
    }
}
}

kernel_fn! {
pub(super) fn reply_files_paid(n: Node, union: Footprint) -> (result: ReplyChange) [
    requires node_headroom(n),
    ensures result == (ReplyChange::Done {
        node: reply_family_values::files_paid(n, union), note: None }),
] {
    let mut node = n;
    let footprint = union;
    node.credit = spend_v(node.credit, 1);
    node.footprint = footprint;
    node.holds = false;
    node = set_phase_v(node, Phase::Ready(Resume::Making));
    let result = ReplyChange::Done { node, note: None };
    result
}
}

kernel_fn! {
pub(super) fn reply_files_ask(n: Node, union: Footprint) -> (result: ReplyChange) [
    ensures result == (ReplyChange::Ask { node: reply_family_values::files_ask(n, union),
        kind: QKind::OutOfCredit, cont: Cont::Ready(Resume::Making), cost: 1 }),
] {
    let mut node = n;
    let footprint = union;
    node.footprint = footprint;
    let result = ReplyChange::Ask { node, kind: QKind::OutOfCredit,
        cont: Cont::Ready(Resume::Making), cost: 1, };
    result
}
}

kernel_fn! {
/// Reply rows / #771: partition the unchanged match for bounded solver queries.
pub(super) fn reply_transition(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox, report: &mut Report) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(reply is MakerNeedsFiles),
    ensures report::forwards(old(report).entries@, final(report).entries@,
            report::round_entries(NodeId(i as u64), report::work_failure(reply))),
        port_t860_contracts::transition_post(*old(w), *final(w), i as int, reply, *old(out), *final(out)),
        progress_gate_contracts::maker_transition(*old(w), *final(w), i as int,
            reply, *old(out), *final(out)),
        (successful_reply_input(old(w).nodes@[i as int], reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint
            || match reply { Reply::PlanLeaf { footprint, .. } => final(w).nodes@[i as int].footprint == footprint, _ => false },
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        (matches!(reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit)
            && old(w).nodes@[i as int].phase is Planning ==> {
                let n = final(w).nodes@[i as int];
                n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
                    && n.review_stop == Some(ReviewStop::PlanCantMeet)
                    && n.credit == old(w).nodes@[i as int].credit
                    && n.plan_retries == old(w).nodes@[i as int].plan_retries
                    && final(w).next_question == old(w).next_question
                    && *final(out) == *old(out)
            }),
        (reply == Reply::AuditDisagrees && old(w).nodes@[i as int].phase == Phase::Auditing
            && old(w).nodes@[i as int].audit_rounds >= 1 ==> {
                let n = final(w).nodes@[i as int];
                n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
                    && n.review_stop == Some(ReviewStop::AuditDisagreement)
                    && n.credit == old(w).nodes@[i as int].credit
                    && final(w).next_question == old(w).next_question
                    && *final(out) == *old(out)
            }),
        !(reply is MakerNeedsFiles) && handler_safety::fail_fast_admitted(old(w).nodes@[i as int], reply)
            ==> handler_safety::ladder(old(w).fail_fast, handler_safety::admitted_retry(old(w).nodes@[i as int], reply),
                old(w).nodes@[i as int], final(w).nodes@[i as int], *old(out), *final(out)),
] proof [reveal(port_t860_contracts::transition_post); reveal(progress_gate_contracts::maker_transition); reveal(distance);] {
    kernel_snapshot! { before_report = report.entries@ }
    kernel_proof! { report::lemmas::work_kind(reply); }
    match reply {
        Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees
            | Reply::AuditDisagrees | Reply::ProtectedPaths => reply_ingest(w, i, reply, out),
        Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet => reply_plan(w, i, reply, out),
        Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit => reply_plan_check(w, i, reply, out),
        _ => {
            reply_work(w, i, reply, out, report);
            kernel_proof! { report::lemmas::round_delta(before_report@, report.entries@,
                NodeId(i as u64), report::work_failure(reply)); }
        },
    }
    kernel_proof! { report::lemmas::forward_round(before_report@, report.entries@,
        NodeId(i as u64), report::work_failure(reply)); }

}
}

kernel_fn! {
/// Rejected reply rows / #860: verbatim failure-handler call under the standing refactor rule.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn on_rejected_reply(w: &mut World, i: usize, _generation: u64, _reply: Reply,
    out: &mut Outbox, report: &mut Report) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
        footprint::valid_footprint(*old(w), old(w).nodes@[i as int].footprint),
        footprint::ordered_table(*old(w)),
        !(old(w).nodes@[i as int].phase is Stopping),
        port_t860_contracts::active(old(w).nodes@[i as int], _generation),
        !port_t860_contracts::admissible(*old(w), old(w).nodes@[i as int], _reply),
    ensures report::forwards(old(report).entries@, final(report).entries@,
            report::round_entries(NodeId(i as u64), Some(FailedRoundCause::RunFailed))),
        port_t860_contracts::reply_post(*old(w), *final(w), i as int, _generation, _reply, *old(out), *final(out)),
        progress_gate_contracts::maker_reply(*old(w), *final(w), i as int,
            _generation, _reply, *old(out), *final(out)),
        (node_checked(*old(w), old(w).nodes@[i as int])
            && _generation == old(w).nodes@[i as int].generation
            && successful_reply_input(old(w).nodes@[i as int], _reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        footprint::valid_footprint(*final(w), final(w).nodes@[i as int].footprint),
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start as int <= old(w).nodes@[i as int].credit.start as int + CREDIT_START_STEP_BUDGET,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        footprint::ordered_table(*final(w)),
        final(w).file_sets@.len() >= old(w).file_sets@.len(),
        forall|j: int| 0 <= j < old(w).file_sets@.len()
            ==> final(w).file_sets@[j] == old(w).file_sets@[j],
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        (matches!(_reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit)
            && old(w).nodes@[i as int].phase is Planning
            && current_item(old(w).nodes@[i as int]) == Some(Item::Await)
            && old(w).nodes@[i as int].run == Some(_generation) ==> {
                let n = final(w).nodes@[i as int];
                n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
                    && n.review_stop == Some(ReviewStop::PlanCantMeet)
                    && n.credit == old(w).nodes@[i as int].credit
                    && n.plan_retries == old(w).nodes@[i as int].plan_retries
                    && final(w).next_question == old(w).next_question
                    && *final(out) == *old(out)
            }),
        (_reply == Reply::AuditDisagrees && old(w).nodes@[i as int].phase == Phase::Auditing
            && old(w).nodes@[i as int].audit_rounds >= 1
            && current_item(old(w).nodes@[i as int]) == Some(Item::Await)
            && old(w).nodes@[i as int].run == Some(_generation) ==> {
                let n = final(w).nodes@[i as int];
                n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
                    && n.review_stop == Some(ReviewStop::AuditDisagreement)
                    && n.credit == old(w).nodes@[i as int].credit
                    && final(w).next_question == old(w).next_question
                    && *final(out) == *old(out)
            }),
        final(w).nodes@[i as int].review_stop == old(w).nodes@[i as int].review_stop,
] {
    kernel_snapshot! { before_report = report.entries@ }
    kernel_snapshot! { before_reply = *w }
    kernel_snapshot! { before_outputs = *out }
    kernel_proof! { reveal(port_t860_contracts::admissible);
        if successful_reply_input(w.nodes@[i as int], _reply) {
            if let Reply::PlanLeaf { footprint, .. } = _reply {
                if footprint::footprint_empty(w.nodes@[i as int].footprint) { footprint::inline_is_valid(*w, 1); }
            }
        }
    }
        run_failed(w, i, out, report);
        kernel_proof! {
            handler_safety::failed_review_stop(before_reply@.nodes@[i as int], w.nodes@[i as int], before_outputs@, *out);
        }
        kernel_proof! {
            report::lemmas::read_failed_round(before_report@, report.entries@, NodeId(i as u64), FailedRoundCause::RunFailed);
            report::lemmas::forward_round(before_report@, report.entries@, NodeId(i as u64), Some(FailedRoundCause::RunFailed));
        }
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        kernel_proof! {
            progress_gate_contracts::table_order_frame(before_reply@, *w);
            progress_gate_contracts::maker_reply_inactive(before_reply@, *w, i as int,
                _generation, _reply, before_outputs@, *out);
        }
        kernel_proof! {
            port_t860_link::rejected(before_reply@, *w, i as int, _generation, _reply, before_outputs@, *out);
        }

}
}

kernel_fn! {
/// Stopping reply rows / #860: verbatim ownership release under the standing refactor rule.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn on_stopping_reply(w: &mut World, i: usize, generation: u64, _reply: Reply,
    _out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(_out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
        footprint::valid_footprint(*old(w), old(w).nodes@[i as int].footprint),
        footprint::ordered_table(*old(w)),
        old(w).nodes@[i as int].phase is Stopping,
    ensures port_t860_contracts::reply_post(*old(w), *final(w), i as int, generation, _reply, *old(_out), *final(_out)),
        progress_gate_contracts::maker_reply(*old(w), *final(w), i as int,
            generation, _reply, *old(_out), *final(_out)),
        (node_checked(*old(w), old(w).nodes@[i as int])
            && generation == old(w).nodes@[i as int].generation
            && successful_reply_input(old(w).nodes@[i as int], _reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        footprint::valid_footprint(*final(w), final(w).nodes@[i as int].footprint),
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 2,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start as int <= old(w).nodes@[i as int].credit.start as int + CREDIT_START_STEP_BUDGET,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added == old(w).nodes@[i as int].credit.added,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        footprint::ordered_table(*final(w)),
        final(w).file_sets@.len() >= old(w).file_sets@.len(),
        forall|j: int| 0 <= j < old(w).file_sets@.len()
            ==> final(w).file_sets@[j] == old(w).file_sets@[j],
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).dependencies@ == old(w).dependencies@,
        final(_out).len <= old(_out).len + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        (matches!(_reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit)
            && old(w).nodes@[i as int].phase is Planning
            && current_item(old(w).nodes@[i as int]) == Some(Item::Await)
            && old(w).nodes@[i as int].run == Some(generation) ==> {
                let n = final(w).nodes@[i as int];
                n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
                    && n.review_stop == Some(ReviewStop::PlanCantMeet)
                    && n.credit == old(w).nodes@[i as int].credit
                    && n.plan_retries == old(w).nodes@[i as int].plan_retries
                    && final(w).next_question == old(w).next_question
                    && *final(_out) == *old(_out)
            }),
        (_reply == Reply::AuditDisagrees && old(w).nodes@[i as int].phase == Phase::Auditing
            && old(w).nodes@[i as int].audit_rounds >= 1
            && current_item(old(w).nodes@[i as int]) == Some(Item::Await)
            && old(w).nodes@[i as int].run == Some(generation) ==> {
                let n = final(w).nodes@[i as int];
                n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
                    && n.review_stop == Some(ReviewStop::AuditDisagreement)
                    && n.credit == old(w).nodes@[i as int].credit
                    && final(w).next_question == old(w).next_question
                    && *final(_out) == *old(_out)
            }),
        final(w).nodes@[i as int].review_stop == old(w).nodes@[i as int].review_stop,
] {
    kernel_snapshot! { before_reply = *w }
    kernel_snapshot! { before_stopping_node = w.nodes@[i as int] }
    kernel_snapshot! { before_outputs = *_out }
        // The run or job ended on its own, so teardown needn't stop it.
        w.nodes[i] = reply_stopping_node(w.nodes[i], generation);
        kernel_proof! {
            assert(port_t860_contracts::stopping_post(before_stopping_node@, w.nodes@[i as int], generation)) by {
                reveal(port_t860_contracts::stopping_post);
            }
        }
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        kernel_proof! {
            progress_gate_contracts::table_order_frame(before_reply@, *w);
            progress_gate_contracts::maker_reply_inactive(before_reply@, *w, i as int,
                generation, _reply, before_outputs@, *_out);
        }
        kernel_proof! {
            port_t860_link::inactive(before_reply@, *w, i as int, generation, _reply, before_outputs@, *_out);
        }

}
}

kernel_fn! {
/// Reply completion as a value, before the admitted-path dispatcher.
pub(super) fn reply_end_node(n: Node) -> (result: Node) [
    ensures port_t860_contracts::ended_post(n, result),
        result == (Node { run: None, job: None, pool: false,
        run_failures: 0, run_incidents: 0,
        effect: match n.effect {
            Some(e) => Some(InFlight { cancelling: e.cancelling
                || matches!(e.kind, EffectKind::StartRun(_)
                    | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }),
            None => None,
        }, ..n }),
        result.repo == n.repo, result.footprint == n.footprint,
        (settle_shape(n) ==> settle_shape(result)),
        result.run.is_none(), result.job.is_none(), !result.pool,
        result.run_failures == 0, result.run_incidents == 0,
        result.phase == n.phase, result.pos == n.pos,
        result.generation == n.generation, result.next_seq == n.next_seq,
        result.credit == n.credit, result.question == n.question,
        result.ticket == n.ticket,
        result.footprint == n.footprint,
        result.last_cmd == n.last_cmd,
        result.fx_failures == n.fx_failures,
        result.fx_incidents == n.fx_incidents,

        result == port_t860_contracts::ended(n),
        transition_safety::node_step(n, result),
] proof [reveal(port_t860_contracts::ended);] {
    let mut m = n;
    end_operation(&mut m);
    m
}
}

kernel_fn! {
/// Late stopping replies release only matching ownership, in the original order.
pub(super) fn reply_stopping_node(n: Node, generation: u64) -> (result: Node) [
    ensures result == port_t860_contracts::stopping(n, generation),
        port_t860_contracts::stopping_post(n, result, generation),
        transition_safety::node_step(n, result),
        result.next_seq == n.next_seq,
] proof [reveal(port_t860_contracts::stopping_post);] {
    let mut node = n;
    let m = &mut node;
    release_run(m, generation);
    release_job(m, generation);
    node
}
}

kernel_fn! {
/// Stopping ownership rows / #860: verbatim run release under the standing refactor rule.
fn release_run(m: &mut Node, generation: u64) -> (result: ()) [
    ensures *final(m) == (Node { run: if old(m).run == Some(generation) { None } else { old(m).run },
            pool: old(m).run != Some(generation) && old(m).pool, ..*old(m) }),
] {
        if m.run == Some(generation) {
            m.run = None;
            m.pool = false;
        }
}
}

kernel_fn! {
/// Stopping ownership rows / #860: verbatim job release under the standing refactor rule.
fn release_job(m: &mut Node, generation: u64) -> (result: ()) [
    ensures *final(m) == (Node { job: if old(m).job == Some(generation) { None } else { old(m).job }, ..*old(m) }),
] {
        if m.job == Some(generation) {
            m.job = None;
        }
}
}
