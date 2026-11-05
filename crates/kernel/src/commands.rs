//! Executable commands transitions and their #771 safety contracts.
use super::*;

kernel_fn! {
/// Command rows / #771: repeated IDs stutter; accepted IDs are assigned verbatim.
/// Freshness is enforced by controller constructors, not by this raw kernel API.
pub(super) fn on_command(w: &mut World, i: usize, id: u64, command: Command,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added as int <= old(w).nodes@[i as int].credit.added as int + 255,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).nodes@[i as int].last_cmd == if id > old(w).nodes@[i as int].last_cmd {
            id
        } else { old(w).nodes@[i as int].last_cmd },
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] {
    if id <= w.nodes[i].last_cmd {
        return; // a repeated command
    }
    w.nodes[i].last_cmd = id;
    match command {
        Command::Stop => enter_stopping(&mut w.nodes[i], Outcome::Stopped),
        Command::ChangeRequest => enter_stopping(&mut w.nodes[i], Outcome::Replaced),
        Command::Pause => {
            let m = &mut w.nodes[i];
            if !m.phase.is_stopping() && !m.phase.is_closed() {
                m.paused = true;
            }
        }
        Command::Resume => w.nodes[i].paused = false,
        Command::Answer { question, answer } => on_answer(w, i, question, answer, out),
    }
}
}

kernel_fn! {
/// Question answer rows / #771: cleanup, continuation and funding permissions.
pub(super) fn allowed(kind: QKind, answer: Answer) -> (result: bool) [
    ensures result == match (kind, answer) {
        (QKind::CleanupFailed, Answer::Retry | Answer::MarkHandled) => true,
        (QKind::CleanupFailed, _) => false,
        (_, Answer::ChangeRequest | Answer::Stop) => true,
        (QKind::AuditDisagreement | QKind::EffectFailed, Answer::Continue) => true,
        (QKind::CantMeet, Answer::TryAgain) => true,
        (QKind::OutOfCredit, Answer::AddCredit(_)) => true,
        _ => false,
    },
] {
    match (kind, answer) {
        (QKind::CleanupFailed, Answer::Retry | Answer::MarkHandled) => true,
        (QKind::CleanupFailed, _) => false,
        (_, Answer::ChangeRequest | Answer::Stop) => true,
        (QKind::AuditDisagreement | QKind::EffectFailed, Answer::Continue) => true,
        (QKind::CantMeet, Answer::TryAgain) => true,
        (QKind::OutOfCredit, Answer::AddCredit(_)) => true,
        _ => false,
    }
}
}

kernel_fn! {
/// Question answer rows / #771: one clarification or one paid continuation.
pub(super) fn on_answer(w: &mut World, i: usize, id: u64, answer: Answer,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added as int <= old(w).nodes@[i as int].credit.added as int + 255,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] {
    let Some(q) = w.nodes[i].question else { return };
    if q.id != id {
        return; // an answer to a question that is no longer open
    }
    match w.nodes[i].phase {
        Phase::Stopping(_) => match answer {
            Answer::Retry => w.nodes[i].question = None,
            Answer::MarkHandled => {
                let m = &mut w.nodes[i];
                m.question = None;
                mark_handled(m);
            }
            _ => {}
        },
        Phase::AwaitingRob => {
            if answer == Answer::Unclear {
                // One clarifying question. After that the question just stays open.
                if !q.clarified {
                    let next = w.next_question;
                    w.next_question += 1;
                    w.nodes[i].question = Some(Question {
                        id: next,
                        clarified: true,
                        ..q
                    });
                    notice(out, i, Notice::Ask { question: next });
                }
                return;
            }
            if !allowed(q.kind, answer) {
                return;
            }
            match answer {
                Answer::Continue => go_on(w, i, q, 0, out),
                Answer::TryAgain => {
                    let m = &mut w.nodes[i];
                    m.attempt = 1;
                    m.review_restarted = false;
                    m.round = 0;
                    m.replanned = false;
                    go_on(
                        w,
                        i,
                        Question {
                            cost: q.cost.max(1),
                            ..q
                        },
                        0,
                        out,
                    );
                }
                Answer::AddCredit(k) => {
                    go_on(w, i, q, k, out);
                }
                Answer::ChangeRequest => enter_stopping(&mut w.nodes[i], Outcome::Replaced),
                Answer::Stop => enter_stopping(&mut w.nodes[i], Outcome::Stopped),
                _ => {}
            }
        }
        _ => {}
    }
}
}

kernel_fn! {
/// Question continuation rows / #771: fund and pay before changing phase.
pub(super) fn go_on(w: &mut World, i: usize, q: Question, deposit: u8,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX, q.cost <= 1,
        old(w).nodes@[i as int].generation < u64::MAX,
        old(w).nodes@[i as int].credit.start as int
            + old(w).nodes@[i as int].credit.earned as int
            + old(w).nodes@[i as int].credit.added as int
                >= old(w).nodes@[i as int].credit.spent as int,
        old(w).nodes@[i as int].credit.withheld ==> q.cost == 0,
        old(w).nodes@[i as int].credit.added as int + 255 <= u64::MAX as int,
        old(w).nodes@[i as int].credit.spent < u64::MAX,
    ensures final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added as int <= old(w).nodes@[i as int].credit.added as int + 255,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(w).nodes@[i as int].question.is_none(),
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] {
    let m = &mut w.nodes[i];
    m.credit.fund_and_pay(deposit, q.cost);
    m.question = None;
    match q.cont {
        Cont::Ingesting => set_phase(m, Phase::Ingesting),
        Cont::Auditing => set_phase(m, Phase::Auditing),
        Cont::Unplanned => set_phase(m, Phase::Unplanned),
        Cont::Planning { must_split } => {
            m.plan_retries = 0;
            set_phase(m, Phase::Planning { must_split });
            if !must_split && m.replanned && q.kind == QKind::OutOfCredit {
                notice(out, i, Notice::Note(NoteKind::Replanned));
            }
        }
        Cont::Ready(r) => set_phase(m, Phase::Ready(r)),
        Cont::Stop(o) => enter_stopping(m, o),
        Cont::Nowhere => enter_stopping(m, Outcome::Stopped),
    }
}
}

kernel_fn! {
/// Cleanup question row / #771: handling cleanup allocates no monotone counter.
pub(super) fn mark_handled(m: &mut Node) -> (result: ()) [
    ensures final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation == old(m).generation,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).question == old(m).question,
        final(m).phase == old(m).phase,
        final(m).last_cmd == old(m).last_cmd,
] {
    if let Some(Item::Teardown(kind)) = m.current() {
        match kind {
            EffectKind::StopRun => {
                m.run = None;
                m.pool = false;
            }
            EffectKind::WithdrawJob => m.job = None,
            EffectKind::ClosePr => m.pr_open = false,
            EffectKind::RemoveWorktree => m.worktree = false,
            _ => {}
        }
        m.pos += 1;
    }
}
}
