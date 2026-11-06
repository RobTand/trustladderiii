//! Executable command safety contracts, split to bound each required query.
use super::*;

kernel_fn! {
/// Question continuation rows / #771: fund and pay before changing phase.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
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
    ensures port_answers_continuation::continuation_post(old(w).nodes@[i as int], final(w).nodes@[i as int], q, deposit),
        (retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out))),
        (old(w).nodes@[i as int].phase == Phase::AwaitingRob
            && old(w).nodes@[i as int].question == Some(q) && !old(w).nodes@[i as int].paused
            && !old(w).nodes@[i as int].waiting_incident && old(w).nodes@[i as int].effect.is_none()
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
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
        final(w).nodes@[i as int].question.is_none(),
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] hide [settle_shape] {
    kernel_snapshot! { before = w.nodes@[i as int] }
    let m = &mut w.nodes[i];
    pay_answer(m, q, deposit);
    kernel_snapshot! { paid = *m }
    resume_answer(m, i, q, out);
    kernel_proof! { port_answers_continuation::compose(before@, paid@, *m, q, deposit);
        port_answers_continuation::legacy_fields(before@, *m, q, deposit);
        port_answers_continuation::legacy_shape(before@, *m, q, deposit);
        if before@.phase == Phase::AwaitingRob && before@.question == Some(q)
            && !before@.paused && !before@.waiting_incident && before@.effect.is_none() {
            port_answers_continuation::legacy_progress(before@, *m, q, deposit);
        } }
}
}

kernel_fn! {
/// Q18 / #806: verbatim funding followed by closing the answered question.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn pay_answer(m: &mut Node, q: Question, deposit: u8) -> (result: ()) [
    requires q.cost <= 1,
        old(m).credit.start as int + old(m).credit.earned as int + old(m).credit.added as int
            >= old(m).credit.spent as int,
        old(m).credit.withheld ==> q.cost == 0,
        old(m).credit.added as int + 255 <= u64::MAX as int,
        old(m).credit.spent < u64::MAX,
    ensures port_answers_continuation::pay_post(*old(m), *final(m), q, deposit),
        final(m).generation == old(m).generation,
] {
    kernel_snapshot! { before = *m }
    m.credit.fund_and_pay(deposit, q.cost);
    m.question = None;
    kernel_proof! { port_answers_continuation::seal_pay(before@, *m, q, deposit); }
}
}

kernel_fn! {
/// AwaitingRob / #806: verbatim continuation dispatch after funding and question clear.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn resume_answer(m: &mut Node, i: usize, q: Question, out: &mut Outbox) -> (result: ()) [
    requires old(m).generation < u64::MAX, old(out).len < OUTBOX,
    ensures port_answers_continuation::resume_post(*old(m), *final(m), q),
        (retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out))),
        final(out).len <= old(out).len + 1,
] {
    match q.cont {
        Cont::Ingesting => {
            resume_ingesting(m, q);
        },
        Cont::Auditing => {
            resume_auditing(m, q);
        },
        Cont::Unplanned => {
            resume_unplanned(m, q);
        },
        Cont::Planning { must_split } => {
            resume_planning(m, i, q, must_split, out);
        }
        Cont::Ready(r) => {
            resume_ready(m, q, r);
        },
        Cont::Stop(o) => {
            resume_stop(m, q, o);
        },
        Cont::Nowhere => {
            resume_nowhere(m, q);
        },
    }
}
}

kernel_fn! {
/// Planning continuation / #806: the verbatim retry reset, phase change and notice.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn resume_planning(m: &mut Node, i: usize, q: Question, must_split: bool,
    out: &mut Outbox) -> (result: ()) [
    requires old(m).generation < u64::MAX, old(out).len < OUTBOX,
        q.cont == (Cont::Planning { must_split }),
    ensures port_answers_continuation::resume_post(*old(m), *final(m), q),
        (retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out))),
        final(out).len <= old(out).len + 1,
] {
    kernel_snapshot! { before = *m }
            m.plan_retries = 0;
            set_phase(m, Phase::Planning { must_split });
            if !must_split && m.replanned && q.kind == QKind::OutOfCredit {
                notice(out, i, Notice::Note(NoteKind::Replanned));
            }
            kernel_proof! { port_answers_continuation::seal_resume(before@, *m, q); }
}
}

kernel_fn! {
/// #806 ingesting continuation: isolate the verbatim primitive call from the dispatch join.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn resume_ingesting(m: &mut Node, _q: Question) -> (result: ()) [
    requires old(m).generation < u64::MAX, _q.cont == (Cont::Ingesting),
    ensures port_answers_continuation::resume_post(*old(m), *final(m), _q),
] {
    kernel_snapshot! { before = *m }
    set_phase(m, Phase::Ingesting);
    kernel_proof! { port_answers_continuation::seal_resume(before@, *m, _q); }
}
}

kernel_fn! {
/// #806 auditing continuation: isolate the verbatim primitive call from the dispatch join.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn resume_auditing(m: &mut Node, _q: Question) -> (result: ()) [
    requires old(m).generation < u64::MAX, _q.cont == (Cont::Auditing),
    ensures port_answers_continuation::resume_post(*old(m), *final(m), _q),
] {
    kernel_snapshot! { before = *m }
    set_phase(m, Phase::Auditing);
    kernel_proof! { port_answers_continuation::seal_resume(before@, *m, _q); }
}
}

kernel_fn! {
/// #806 unplanned continuation: isolate the verbatim primitive call from the dispatch join.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn resume_unplanned(m: &mut Node, _q: Question) -> (result: ()) [
    requires old(m).generation < u64::MAX, _q.cont == (Cont::Unplanned),
    ensures port_answers_continuation::resume_post(*old(m), *final(m), _q),
] {
    kernel_snapshot! { before = *m }
    set_phase(m, Phase::Unplanned);
    kernel_proof! { port_answers_continuation::seal_resume(before@, *m, _q); }
}
}

kernel_fn! {
/// #806 ready continuation: isolate the verbatim primitive call from the dispatch join.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn resume_ready(m: &mut Node, _q: Question, r: Resume) -> (result: ()) [
    requires old(m).generation < u64::MAX, _q.cont == (Cont::Ready(r)),
    ensures port_answers_continuation::resume_post(*old(m), *final(m), _q),
] {
    kernel_snapshot! { before = *m }
    set_phase(m, Phase::Ready(r));
    kernel_proof! { port_answers_continuation::seal_resume(before@, *m, _q); }
}
}

kernel_fn! {
/// #806 stop continuation: isolate the verbatim primitive call from the dispatch join.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn resume_stop(m: &mut Node, _q: Question, o: Outcome) -> (result: ()) [
    requires old(m).generation < u64::MAX, _q.cont == (Cont::Stop(o)),
    ensures port_answers_continuation::resume_post(*old(m), *final(m), _q),
] {
    kernel_snapshot! { before = *m }
    enter_stopping(m, o);
    kernel_proof! { port_answers_continuation::seal_resume(before@, *m, _q); }
}
}

kernel_fn! {
/// #806 nowhere continuation: isolate the verbatim primitive call from the dispatch join.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn resume_nowhere(m: &mut Node, _q: Question) -> (result: ()) [
    requires old(m).generation < u64::MAX, _q.cont == (Cont::Nowhere),
    ensures port_answers_continuation::resume_post(*old(m), *final(m), _q),
] {
    kernel_snapshot! { before = *m }
    enter_stopping(m, Outcome::Stopped);
    kernel_proof! { port_answers_continuation::seal_resume(before@, *m, _q); }
}
}
