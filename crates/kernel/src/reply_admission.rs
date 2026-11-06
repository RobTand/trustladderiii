//! Executable reply admission transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
/// Reply rows / #771: phase, schema and commit identity use the executable checks.
pub(super) fn accepts(w: &World, n: &Node, reply: &Reply) -> (result: bool) [
    ensures result == port_review_reply::accepted_reply(*w, *n, *reply),
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
] proof [reveal(port_review_reply::accepted_reply);] {
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
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
        footprint::valid_footprint(*old(w), old(w).nodes@[i as int].footprint),
        footprint::ordered_table(*old(w)),
    ensures port_review_reply::reply_post(*old(w), *final(w), i as int,
            generation, reply, *old(out), *final(out)),
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
] {
    kernel_snapshot! { before_reply = *w }
    kernel_snapshot! { before_outputs = *out }
    let n = &w.nodes[i];
    if n.phase.is_stopping() {
        // The run or job ended on its own, so teardown needn't stop it.
        let m = &mut w.nodes[i];
        if m.run == Some(generation) {
            m.run = None;
            m.pool = false;
        }
        if m.job == Some(generation) {
            m.job = None;
        }
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        kernel_proof! {
            progress_gate_contracts::table_order_frame(before_reply@, *w);
            progress_gate_contracts::maker_reply_inactive(before_reply@, *w, i as int,
                generation, reply, before_outputs@, *out);
        }
        kernel_proof! {
            port_review_admission::inactive(before_reply@, *w, i as int, generation, reply, before_outputs@, *out);
            port_review_admission::mount(before_reply@, *w, i as int, generation, reply, before_outputs@, *out);
        }
        return;
    }
    if n.current() != Some(Item::Await) || (n.run != Some(generation) && n.job != Some(generation))
    {
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        kernel_proof! {
            progress_gate_contracts::table_order_frame(before_reply@, *w);
            progress_gate_contracts::maker_reply_inactive(before_reply@, *w, i as int,
                generation, reply, before_outputs@, *out);
        }
        kernel_proof! {
            port_review_admission::inactive(before_reply@, *w, i as int, generation, reply, before_outputs@, *out);
            port_review_admission::mount(before_reply@, *w, i as int, generation, reply, before_outputs@, *out);
        }
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
        run_failed(w, i, out);
        kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint); }
        kernel_proof! {
            progress_gate_contracts::table_order_frame(before_reply@, *w);
            progress_gate_contracts::maker_reply_inactive(before_reply@, *w, i as int,
                generation, reply, before_outputs@, *out);
        }
        kernel_proof! {
            port_review_admission::rejected(before_reply@, *w, i as int, generation, reply, before_outputs@, *out);
            port_review_admission::mount(before_reply@, *w, i as int, generation, reply, before_outputs@, *out);
        }
        return;
    }
    admitted_reply(w, i, generation, reply, out);

}
}

kernel_fn! {
/// Reply rows / #806: the accepted statement block moved verbatim under the standing rule.
fn admitted_reply(w: &mut World, i: usize, _generation: u64, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
        footprint::valid_footprint(*old(w), old(w).nodes@[i as int].footprint),
        footprint::ordered_table(*old(w)),
        !(old(w).nodes@[i as int].phase is Stopping),
        port_review_reply::active(old(w).nodes@[i as int], _generation),
        port_review_reply::accepted_reply(*old(w), old(w).nodes@[i as int], reply),
    ensures port_review_reply::reply_post(*old(w), *final(w), i as int,
            _generation, reply, *old(out), *final(out)),
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
] proof [reveal(port_review_reply::accepted_reply);] {
    kernel_snapshot! { before_reply = *w }
    kernel_snapshot! { before_outputs = *out }
    end_operation(&mut w.nodes[i]);
    kernel_proof! {
        footprint::valid_under_table_growth(before_reply@, *w, w.nodes@[i as int].footprint);
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
            kernel_proof! { footprint::valid_under_table_growth(before_reply@, *w, files); }
            reply_files(w, i, files, fits, out);
            kernel_proof! {
                progress_gate_contracts::maker_reply_inactive(before_reply@, *w, i as int,
                    _generation, reply, before_outputs@, *out);
            }
        }
        _ => {
            reply_transition(w, i, reply, out);
            kernel_proof! {
                progress_gate_contracts::reply_bounds_result(after_operation@, *w, i as int,
                    before_outputs@.len, out.len);
                progress_gate_contracts::table_order_frame(after_operation@, *w);
                progress_gate_contracts::maker_reply_composed(before_reply@, after_operation@,
                    *w, i as int, _generation, reply, before_outputs@, *out);
                if let Reply::PlanLeaf { footprint, .. } = reply {
                    footprint::valid_under_table_growth(before_reply@, *w, footprint);
                }
                footprint::valid_under_table_growth(before_reply@, *w, before_reply@.nodes@[i as int].footprint);
            }
        }
    }    kernel_proof! {
        if let Reply::MakerNeedsFiles { .. } = reply {
            port_review_admission::unselected(after_operation@, *w, i as int,
                after_operation@.nodes@[i as int], reply, before_outputs@, *out);
        }
        port_review_admission::accepted(before_reply@, after_operation@, *w, i as int,
            _generation, reply, before_outputs@, *out);
        port_review_admission::mount(before_reply@, *w, i as int, _generation, reply, before_outputs@, *out);
    }

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
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
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
] proof [reveal(retry_bounds); reveal(footprint::ordered_table);] {
    kernel_snapshot! { before_files = *w }
    if fits && !files.is_empty() && w.nodes[i].credit.can_spend(1) {
        let footprint = w.union_footprints(w.nodes[i].footprint, files);
        kernel_snapshot! { after_union = *w }
        let m = &mut w.nodes[i];
        m.credit.spend(1);
        m.footprint = footprint;
        m.holds = false;
        set_phase(m, Phase::Ready(Resume::Making));
        kernel_proof! { footprint::valid_under_table_growth(after_union@, *w, footprint); }
    } else if fits && !files.is_empty() && w.fail_fast {
        // Q54 stops retries, not credit decisions: Rob funds the extension.
        let footprint = w.union_footprints(w.nodes[i].footprint, files);
        kernel_snapshot! { after_union = *w }
        w.nodes[i].footprint = footprint;
        ask_rob(
            w,
            i,
            QKind::OutOfCredit,
            Cont::Ready(Resume::Making),
            1,
            out,
        );
        kernel_proof! { footprint::valid_under_table_growth(after_union@, *w, footprint); }
    } else {
        out_of_room(w, i, ReviewStop::MakerNeedsFiles, out);
        kernel_proof! { footprint::valid_under_table_growth(before_files@, *w, w.nodes@[i as int].footprint); }
    }
    }
}

kernel_fn! {
/// Reply rows / #771: partition the unchanged match for bounded solver queries.
pub(super) fn reply_transition(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !(reply is MakerNeedsFiles),
    ensures port_review_reply::transition_post(*old(w), *final(w), i as int,
            old(w).nodes@[i as int], reply, *old(out), *final(out)),
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
] proof [reveal(port_review_reply::transition_post); reveal(progress_gate_contracts::maker_transition); reveal(distance);] {
    match reply {
        Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees
            | Reply::AuditDisagrees | Reply::ProtectedPaths => reply_ingest(w, i, reply, out),
        Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet => reply_plan(w, i, reply, out),
        Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit => reply_plan_check(w, i, reply, out),
        _ => reply_work(w, i, reply, out),
    }
}
}
