//! Executable command safety contracts, split to bound each required query.
use super::*;

#[cfg(verus_keep_ghost)]
verus! {
/// #806: five independently framed answer rows, with cleanup advancement.
pub(super) open spec fn answer_supported(answer: Answer) -> bool {
    matches!(answer, Answer::ChangeRequest | Answer::Stop | Answer::Unclear | Answer::Retry | Answer::MarkHandled)
}

/// #806: neutral public contract for the real answer handler, including stale IDs.
#[verifier::opaque]
pub(super) open spec fn answer_post(before: Node, after: Node,
    question: u64, answer: Answer, next_before: u64, next_after: u64) -> bool {
    after == port_answers_model::answered(before, question, answer, next_before)
        && next_after == if port_answers_model::clarifies(before, question, answer) {
            (next_before + 1) as u64
        } else { next_before }
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
    ensures answer_supported(answer) ==> answer_post(old(w).nodes@[i as int], final(w).nodes@[i as int],
            id, answer, old(w).next_question, final(w).next_question),
        (answer_supported(answer) && retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out))),
        (node_checked(*old(w), old(w).nodes@[i as int])
            && answer_progress_input(old(w).nodes@[i as int], id, answer)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
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
] proof [reveal(distance);] {
    kernel_snapshot! { before_answer = w.nodes@[i as int] }
    kernel_snapshot! { before_question = w.next_question }
    let Some(q) = w.nodes[i].question else {
        kernel_proof! { if answer_supported(answer) {
            port_answers_handler::post_unchanged(before_answer@, id, answer, before_question@);
        } }
        return
    };
    if q.id != id {
        kernel_proof! { if answer_supported(answer) {
            port_answers_handler::post_unchanged(before_answer@, id, answer, before_question@);
        } }
        return; // an answer to a question that is no longer open
    }
    kernel_proof! { port_answers_handler::cleanup_input(*w, before_answer@, id, answer); }
    match w.nodes[i].phase {
        Phase::Stopping(_) => answer_stopping(w, i, id, answer),
        Phase::AwaitingRob => answer_awaiting(w, i, q, answer, out),
        _ => { kernel_proof! { if answer_supported(answer) {
            port_answers_handler::post_unchanged(before_answer@, id, answer, before_question@);
        } } }
    }
}
}

kernel_fn! {
/// Cleanup question row / #806: the original Stopping answer branch, unchanged.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn answer_stopping(w: &mut World, i: usize, _id: u64, answer: Answer) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(w).nodes@[i as int].phase is Stopping,
        old(w).nodes@[i as int].question.is_some(), old(w).nodes@[i as int].question.unwrap().id == _id,
    ensures answer_supported(answer) ==> answer_post(old(w).nodes@[i as int], final(w).nodes@[i as int],
            _id, answer, old(w).next_question, final(w).next_question),
        (answer == Answer::MarkHandled && old(w).nodes@[i as int].effect.is_none()
            && !old(w).nodes@[i as int].waiting_incident
            && (match current_item(Node { question: None, ..old(w).nodes@[i as int] }) { Some(Item::Teardown(_)) => true, _ => false })
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
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
] hide [current_item, items, settle_shape] {
    kernel_snapshot! { before_answer = w.nodes@[i as int] }
    kernel_snapshot! { before_question = w.next_question }
    kernel_proof! { port_answers_handler::stopping_question(before_answer@); }
    match answer {
            Answer::Retry => w.nodes[i].question = None,
            Answer::MarkHandled => {
                let m = &mut w.nodes[i];
                answer_cleanup(m);
            }
            _ => {}
        }
    kernel_proof! { if answer_supported(answer) {
        if answer == Answer::Retry {
            port_answers_handler::post_retry(before_answer@, _id, before_question@);
        } else if answer == Answer::MarkHandled {
            port_answers_handler::post_handled(before_answer@, _id, before_question@);
        } else {
            port_answers_handler::post_unchanged(before_answer@, _id, answer, before_question@);
        }
    } }
}
}

kernel_fn! {
/// Cleanup question row / #806: isolate the verbatim question-clear and cleanup calls.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn answer_cleanup(m: &mut Node) -> (result: ()) [
    requires old(m).phase is Stopping, old(m).question.is_some(),
    ensures final(m).pos == if matches!(current_item(Node { question: None, ..*old(m) }), Some(Item::Teardown(_))) {
            (old(m).pos + 1) as u8
        } else { old(m).pos },
        *final(m) == port_answers_model::handled(Node { question: None, ..*old(m) }),
        *final(m) == (Node { question: None, pos: final(m).pos, run: final(m).run,
            pool: final(m).pool, job: final(m).job, pr_open: final(m).pr_open,
            worktree: final(m).worktree, ..*old(m) }),
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        (old(m).effect.is_none() && !old(m).waiting_incident
            && matches!(current_item(Node { question: None, ..*old(m) }), Some(Item::Teardown(_))))
            ==> distance(*final(m)) < distance(*old(m)),
] hide [current_item, items, settle_shape] {
    kernel_snapshot! { before_answer = *m }
    kernel_proof! { port_answers_handler::stopping_question(before_answer@); }
    m.question = None;
    kernel_snapshot! { before_cleanup = *m }
    mark_handled(m);
    kernel_proof! { port_answers_handler::handled_result(before_cleanup@, *m); }
}
}

kernel_fn! {
/// AwaitingRob question rows / #806: the original answer branch, unchanged.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn answer_awaiting(w: &mut World, i: usize, q: Question, answer: Answer,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        old(w).nodes@[i as int].phase == Phase::AwaitingRob,
        old(w).nodes@[i as int].question == Some(q),
        node_headroom(old(w).nodes@[i as int]), arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures answer_supported(answer) ==> answer_post(old(w).nodes@[i as int], final(w).nodes@[i as int],
            q.id, answer, old(w).next_question, final(w).next_question),
        (answer_supported(answer) && retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out))),
        (node_checked(*old(w), old(w).nodes@[i as int])
            && answer_progress_input(old(w).nodes@[i as int], q.id, answer)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
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
] proof [reveal(distance);] {
    kernel_snapshot! { before_answer = w.nodes@[i as int] }
    kernel_snapshot! { before_question = w.next_question }
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
                kernel_proof! {
                    port_answers_handler::post_unclear(before_answer@, q.id, before_question@);
                }
                return;
            }
            if !allowed(q.kind, answer) {
                kernel_proof! { if answer_supported(answer) {
                    port_answers_handler::post_unchanged(before_answer@, q.id, answer, before_question@);
                } }
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
    kernel_proof! { if answer_supported(answer) {
        if matches!(answer, Answer::Stop | Answer::ChangeRequest) {
            port_answers_handler::post_stopping(before_answer@, q.id, answer, before_question@);
        } else {
            port_answers_handler::post_unchanged(before_answer@, q.id, answer, before_question@);
        }
    } }
        }
}

#[cfg(test)]
#[path = "answer_equivalence/mod.rs"]
mod equivalence;
