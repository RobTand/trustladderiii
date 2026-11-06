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
    ensures (successful_reply_input(old(w).nodes@[i as int], reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint
            || match reply { Reply::PlanLeaf { footprint, .. } => final(w).nodes@[i as int].footprint == footprint, _ => false },
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        port_replies_contracts::transition_post(*old(w), *final(w), i as int,
            old(w).nodes@[i as int], reply, *old(out), *final(out)),
        (old(w).nodes@[i as int].phase is Planning) ==> port_replies_contracts::node_post(
            old(w).nodes@[i as int], final(w).nodes@[i as int], old(w).next_question, old(w).fail_fast, reply),
        port_replies_contracts::output_post(*old(out), *final(out), i as int,
            port_replies_contracts::emission(old(w).nodes@[i as int], old(w).next_question, old(w).fail_fast, reply)),
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
] proof [reveal(reply_bounds); reveal(retry_bounds);] {
    kernel_snapshot! { worker_before = *w }
    kernel_snapshot! { worker_outputs = *out }
    kernel_proof! { footprint::inline_is_valid(*w, 1); }
    match reply {
        Reply::PlanLeaf { footprint, fits } => {
            let m = &mut w.nodes[i];
            let must_split = matches!(m.phase, Phase::Planning { must_split: true });
            if fits && !footprint.is_empty() && !must_split {
                m.footprint = footprint;
                m.attempt = 1;
                m.review_restarted = false;
                m.round = 0;
                m.plan_retries = 0;
                set_phase(m, Phase::Ready(Resume::Making));
                kernel_proof! { if worker_before@.nodes@[i as int].phase is Planning {
                    port_replies_handlers::leaf_ready_result(worker_before@.nodes@[i as int],
                        w.nodes@[i as int], worker_before@.next_question, worker_before@.fail_fast, footprint, fits);
                } }
            } else {
                plan_retry(w, i, false, out);
                kernel_proof! { if worker_before@.nodes@[i as int].phase is Planning {
                    port_replies_handlers::plan_exact(worker_before@.nodes@[i as int], w.nodes@[i as int],
                        worker_before@.next_question, worker_before@.fail_fast, false);
                    port_replies_handlers::leaf_retry_result(worker_before@.nodes@[i as int],
                        w.nodes@[i as int], worker_before@.next_question, worker_before@.fail_fast, footprint, fits);
                } }
            }
        }
        Reply::PlanSplit => {
            ask_rob(w, i, QKind::NeedsSplitting, Cont::Nowhere, 0, out);
            kernel_proof! { if worker_before@.nodes@[i as int].phase is Planning {
                port_replies_handlers::split_result(worker_before@.nodes@[i as int],
                    w.nodes@[i as int], worker_before@.next_question, worker_before@.fail_fast);
            } }
        },
        Reply::PlanCantMeet => {
            plan_retry(w, i, true, out);
            kernel_proof! { if worker_before@.nodes@[i as int].phase is Planning {
                port_replies_handlers::plan_exact(worker_before@.nodes@[i as int], w.nodes@[i as int],
                    worker_before@.next_question, worker_before@.fail_fast, true);
                port_replies_handlers::cant_result(worker_before@.nodes@[i as int],
                    w.nodes@[i as int], worker_before@.next_question, worker_before@.fail_fast);
            } }
        },
        _ => {}
    }
    kernel_proof! {
        if successful_reply_input(worker_before@.nodes@[i as int], reply) {
            port_replies_handlers::plan_progress(worker_before@.nodes@[i as int], w.nodes@[i as int],
                worker_before@.next_question, worker_before@.fail_fast, reply);
        }
        assert(port_replies_contracts::output_post(worker_outputs@, *out, i as int,
            port_replies_contracts::emission(worker_before@.nodes@[i as int], worker_before@.next_question,
                worker_before@.fail_fast, reply))) by {
            reveal(port_replies_contracts::output_post);
        }
        if retirement::notices_only(worker_outputs@) {
            port_replies_handlers::output_notices(worker_outputs@, *out, i as int,
                port_replies_contracts::emission(worker_before@.nodes@[i as int], worker_before@.next_question,
                    worker_before@.fail_fast, reply));
        }
        assert(port_replies_contracts::transition_post(worker_before@, *w, i as int,
            worker_before@.nodes@[i as int], reply, worker_outputs@, *out)) by {
            reveal(port_replies_contracts::transition_post);
            reveal(port_review_reply::accepted_reply);
            reveal(port_failfast_diversion::plan_retry_post);
        }
    }
}
}

kernel_fn! {
/// Reply rows / #771: one phase change, at most one paid credit and one question.
#[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
pub(super) fn reply_work(w: &mut World, i: usize, reply: Reply,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        !matches!(reply, Reply::MakerNeedsFiles { .. } | Reply::IngestDone { .. }
            | Reply::IngestEarlyEnd(_) | Reply::AuditAgrees | Reply::AuditDisagrees
            | Reply::ProtectedPaths | Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet
            | Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
    ensures port_review_reply::transition_post(*old(w), *final(w), i as int,
            old(w).nodes@[i as int], reply, *old(out), *final(out)),
        progress_gate_contracts::maker_transition(*old(w), *final(w), i as int,
            reply, *old(out), *final(out)),
        (successful_reply_input(old(w).nodes@[i as int], reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(port_review_reply::transition_post); reveal(port_review_reply::transition); reveal(port_review_retry::recheck_post); reveal(progress_gate_contracts::maker_transition); reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(reply_bounds); reveal(retry_bounds);] {
    match reply {
        Reply::MakerDone { head } => {
            let m = &mut w.nodes[i];
            m.head = Some(head);
            m.checked = None;
            set_phase(m, Phase::Checking);
        }
        Reply::MakerCantMeet => next_rung(w, i, ReviewStop::MakerCantMeet, out),
        Reply::MakerOutOfRoom => out_of_room(w, i, ReviewStop::MakerOutOfRoom, out),
        Reply::ChecksDone {
            pass, in_footprint, ..
        } => {
            if pass && in_footprint {
                let m = &mut w.nodes[i];
                m.checked = m.head;
                set_phase(m, Phase::Reviewing);
            } else {
                failed_round(w, i, ReviewStop::ChecksFailed, out);
            }
        }
        Reply::Approved { head } => {
            let m = &mut w.nodes[i];
            m.approved = Some(head);
            set_phase(m, Phase::Publishing);
        }
        Reply::ChangesRequested => failed_round(w, i, ReviewStop::ChangesRequested, out),
        Reply::ReviewerOutOfRoom => {
            if w.nodes[i].review_restarted {
                out_of_room(w, i, ReviewStop::ReviewerOutOfRoom, out);
            } else {
                w.nodes[i].review_restarted = true;
                recheck(w, i, Phase::Reviewing, out);
            }
        }
        _ => reply_rebase(w, i, reply, out),
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
    ensures port_review_reply::transition_post(*old(w), *final(w), i as int,
            old(w).nodes@[i as int], reply, *old(out), *final(out)),
        (successful_reply_input(old(w).nodes@[i as int], reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] hide [successful_reply_input, ordinary_await, current_item, items] proof [reveal(reply_bounds); reveal(retry_bounds);] {
    kernel_snapshot! { rebase_before = *w }
    kernel_snapshot! { rebase_outputs = *out }

    match reply {
        Reply::RebaseClean { head } => {
            update_recheck_head(&mut w.nodes[i], head);
            kernel_snapshot! { rebase_updated = w.nodes@[i as int] }
            recheck(w, i, Phase::Checking, out);
            kernel_proof! {
                port_review_admission::clean_pair(rebase_before@.nodes@[i as int], rebase_updated@,
                    w.nodes@[i as int], head, rebase_before@.fail_fast, rebase_before@.next_question, w.next_question);
            }
        }
        Reply::RebaseNeedsChanges => {
            let m = &mut w.nodes[i];
            if m.credit.can_spend(1) {
                m.credit.spend(1);
                set_phase(m, Phase::Making);
            } else {
                ask_rob(
                    w,
                    i,
                    QKind::OutOfCredit,
                    Cont::Ready(Resume::Making),
                    1,
                    out,
                );
            }
        }
        _ => {}
    }
    kernel_proof! {
        assert(port_review_reply::transition_post(rebase_before@, *w, i as int,
            rebase_before@.nodes@[i as int], reply, rebase_outputs@, *out)) by {
            reveal(port_review_reply::transition_post);
            reveal(port_review_reply::transition);
        }
    }
    kernel_proof! {
        port_review_admission::rebase_progress(rebase_before@, *w, i as int, reply, rebase_outputs@, *out);
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
