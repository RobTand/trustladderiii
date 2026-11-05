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
] proof [reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(reply_bounds); reveal(retry_bounds);] {
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
            } else {
                plan_retry(w, i, false, out);
            }
        }
        Reply::PlanSplit => ask_rob(w, i, QKind::NeedsSplitting, Cont::Nowhere, 0, out),
        Reply::PlanCantMeet => plan_retry(w, i, true, out),
        _ => {}
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
            | Reply::ProtectedPaths | Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet),
    ensures progress_gate_contracts::maker_transition(*old(w), *final(w), i as int,
            reply, *old(out), *final(out)),
        (successful_reply_input(old(w).nodes@[i as int], reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(progress_gate_contracts::maker_transition); reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(reply_bounds); reveal(retry_bounds);] {
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
    ensures (successful_reply_input(old(w).nodes@[i as int], reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(out).len, final(out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
] proof [reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(reply_bounds); reveal(retry_bounds);] {
    match reply {
        Reply::RebaseClean { head } => {
            update_recheck_head(&mut w.nodes[i], head);
            recheck(w, i, Phase::Checking, out);
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
