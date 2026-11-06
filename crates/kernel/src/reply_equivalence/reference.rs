//! #806: pre-refactor on_reply, with its body retained verbatim.
use super::super::*;

kernel_fn! {
pub(super) fn reference_on_reply(w: &mut World, i: usize, generation: u64, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]), old(w).next_question < u64::MAX,
        footprint::valid_footprint(*old(w), old(w).nodes@[i as int].footprint),
        footprint::ordered_table(*old(w)),
    ensures progress_gate_contracts::maker_reply(*old(w), *final(w), i as int,
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
        return;
    }
    kernel_snapshot! { before_operation = *w }
    end_operation(&mut w.nodes[i]);
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
                progress_gate_contracts::maker_reply_inactive(before_reply@, *w, i as int,
                    generation, reply, before_outputs@, *out);
            }
        }
        _ => {
            reply_transition(w, i, reply, out);
            kernel_proof! {
                progress_gate_contracts::reply_bounds_result(after_operation@, *w, i as int,
                    before_outputs@.len, out.len);
                progress_gate_contracts::table_order_frame(after_operation@, *w);
                progress_gate_contracts::maker_reply_composed(before_reply@, after_operation@,
                    *w, i as int, generation, reply, before_outputs@, *out);
                if let Reply::PlanLeaf { footprint, .. } = reply {
                    footprint::valid_under_table_growth(before_operation@, *w, footprint);
                }
                footprint::valid_under_table_growth(before_operation@, *w, before_operation@.nodes@[i as int].footprint);
            }
        }
    }
}
}
