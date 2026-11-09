//! Executable command safety contracts, split to bound each required query.
use super::*;

kernel_fn! {
/// Question answer rows / #771: one clarification or one paid continuation.
/// on_command's answer work, verified against one node.
pub(super) fn answer_node(m: &mut Node, next_question: &mut u64, i: usize, id: u64, answer: Answer,
    out: &mut Outbox) -> (result: ()) [
    requires old(out).len < OUTBOX,
        node_headroom(*old(m)),
        arithmetic_consistent(*old(m)),
        *old(next_question) < u64::MAX,
    ensures (answer_progress_input(*old(m), id, answer)
            ==> distance(*final(m)) < distance(*old(m))),
        (old(m).phase is Closed ==> final(m).phase is Closed),
        final(m).repo == old(m).repo,
        final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).next_seq == old(m).next_seq,
        final(m).credit.start == old(m).credit.start,
        final(m).credit.earned == old(m).credit.earned,
        final(m).credit.added as int <= old(m).credit.added as int + 255,
        final(m).credit.spent as int <= old(m).credit.spent as int + 1,
        final(m).last_cmd == old(m).last_cmd,
        *final(next_question) as int <= *old(next_question) as int + 1,
        final(out).len <= old(out).len + 1,
        final(m).proposed == old(m).proposed,
        final(m).adopted == old(m).adopted,
        (old(m).phase is Closed ==> final(m).phase == old(m).phase),
        handler_safety::command_step(*old(m), *final(m)),
        transition_safety::notice_only(*old(out), *final(out), NodeId(i as u64)),
] proof [reveal(distance);] {
    kernel_snapshot! { before_answer = *m }
    let Some(q) = m.question else {
        kernel_proof! { handler_safety::same_step(before_answer@); }
        return;
    };
    if q.id != id {
        kernel_proof! { handler_safety::same_step(before_answer@); }
        return; // an answer to a question that is no longer open
    }
    match m.phase {
        Phase::Stopping(_) => match answer {
            Answer::Retry => {
                m.question = None;
                kernel_proof! { handler_safety::field_step(before_answer@, *m); }
            }
            Answer::MarkHandled => {
                m.question = None;
                mark_handled(m);
                kernel_proof! { handler_safety::field_step(before_answer@, *m); }
            }
            _ => { kernel_proof! { handler_safety::same_step(before_answer@); } }
        },
        Phase::AwaitingRob => {
            if answer == Answer::Unclear {
                // One clarifying question. After that the question just stays open.
                if !q.clarified {
                    clarify_node(m, next_question, i, q, out);
                }
                kernel_proof! { if q.clarified { handler_safety::same_step(before_answer@); } }
                return;
            }
            if !allowed(q.kind, answer) {
                kernel_proof! { handler_safety::same_step(before_answer@); }
                return;
            }
            match answer {
                Answer::Continue => go_on(m, i, q, 0, out),
                Answer::TryAgain => retry_answer_node(m, i, q, out),
                Answer::AddCredit(k) => {
                    go_on(m, i, q, k, out);
                }
                Answer::ChangeRequest => {
                    kernel_snapshot! { before_replace = *m }
                    enter_stopping(m, Outcome::Replaced);
                    kernel_proof! {
                        assert(m.proposed == before_replace@.proposed && m.adopted == before_replace@.adopted)
                            by { reveal(retirement::stopped); }
                        handler_safety::stop_step(before_replace@, Outcome::Replaced);
                    }
                }
                Answer::Stop => {
                    kernel_snapshot! { before_stop = *m }
                    enter_stopping(m, Outcome::Stopped);
                    kernel_proof! {
                        assert(m.proposed == before_stop@.proposed && m.adopted == before_stop@.adopted)
                            by { reveal(retirement::stopped); }
                        handler_safety::stop_step(before_stop@, Outcome::Stopped);
                    }
                }
                _ => { kernel_proof! { handler_safety::same_step(before_answer@); } }
            }
        }
        _ => { kernel_proof! { handler_safety::same_step(before_answer@); } }
    }
}
}

#[cfg(verus_keep_ghost)]
verus! {
/// TryAgain / #771: the first attempt restarts before the continuation.
pub(super) open spec fn try_again_node(n: Node) -> Node {
    Node { attempt: 1, review_restarted: false, round: 0, replanned: false, ..n }
}
}

kernel_fn! {
/// Clarification rows / #771: one clarifying question under a fresh id.
fn clarify_node(m: &mut Node, next_question: &mut u64, i: usize, q: Question,
    out: &mut Outbox) -> (result: ()) [
    requires *old(next_question) < u64::MAX, old(out).len < OUTBOX,
    ensures *final(m) == (Node {
            question: Some(Question { id: *old(next_question), clarified: true, ..q }), ..*old(m)
        }),
        *final(next_question) == *old(next_question) + 1,
        final(out).len == old(out).len + 1,
        final(out).items@ == old(out).items@.update(old(out).len as int,
            Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Ask { question: *old(next_question) } })),
        final(out).items@[old(out).len as int]
            == Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Ask { question: *old(next_question) } }),
        old(m).phase == Phase::AwaitingRob && old(m).question == Some(q) && !q.clarified
            ==> handler_safety::command_step(*old(m), *final(m)),
] {
    kernel_snapshot! { before_clarify = *m }
    let next = *next_question;
    *next_question += 1;
    m.question = Some(Question {
        id: next,
        clarified: true,
        ..q
    });
    notice(out, i, Notice::Ask { question: next });
    kernel_proof! {
        if before_clarify@.phase == Phase::AwaitingRob && before_clarify@.question == Some(q) && !q.clarified {
            handler_safety::clarify_step(before_clarify@, q, next);
        }
    }
}
}

kernel_fn! {
/// TryAgain rows / #771: restart the first attempt, then continue with at least
/// one credit; the continuation is continue_node, then go_on's replanned note.
fn retry_answer_node(m: &mut Node, i: usize, q: Question, out: &mut Outbox) -> (result: ()) [
    requires old(out).len < OUTBOX, q.cost <= 1,
        old(m).generation < u64::MAX,
        old(m).credit.start as int + old(m).credit.earned as int + old(m).credit.added as int
            >= old(m).credit.spent as int,
        old(m).credit.withheld ==> (if q.cost > 1 { q.cost } else { 1u8 }) == 0,
        old(m).credit.added as int + 255 <= u64::MAX as int,
        old(m).credit.spent < u64::MAX,
    ensures ({
            let b = try_again_node(*old(m));
            let q1 = Question { cost: if q.cost > 1 { q.cost } else { 1u8 }, ..q };
            (b.phase == Phase::AwaitingRob && b.question == Some(q1) && !b.paused
                && !b.waiting_incident && b.effect.is_none()
                ==> distance(*final(m)) < distance(b))
            && final(m).repo == b.repo && final(m).footprint == b.footprint
            && (settle_shape(b) ==> settle_shape(*final(m)))
            && final(m).generation as int <= b.generation as int + 1
            && final(m).next_seq == b.next_seq
            && final(m).credit.start == b.credit.start && final(m).credit.earned == b.credit.earned
            && final(m).credit.added as int <= b.credit.added as int + 255
            && final(m).credit.spent as int <= b.credit.spent as int + 1
            && final(m).last_cmd == b.last_cmd
            && final(m).question.is_none()
            && final(m).proposed == b.proposed && final(m).adopted == b.adopted
            && final(m).review_stop == b.review_stop && final(m).attempt == 1
            && !final(m).replanned
        }),
        old(m).phase == Phase::AwaitingRob ==> handler_safety::command_step(*old(m), *final(m)),
        final(out).len <= old(out).len + 1,
        final(out).len == old(out).len || (final(out).len == old(out).len + 1
            && final(out).items@[old(out).len as int]
                == Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Note(NoteKind::Replanned) })),
] {
    kernel_snapshot! { before_retry = *m }
    m.attempt = 1;
    m.review_restarted = false;
    m.round = 0;
    m.replanned = false;
    continue_node(
        m,
        Question {
            cost: q.cost.max(1),
            ..q
        },
        0,
    );
    match q.cont {
        Cont::Planning { must_split } => {
            if !must_split && m.replanned && q.kind == QKind::OutOfCredit {
                notice(out, i, Notice::Note(NoteKind::Replanned));
            }
        }
        _ => {}
    }
    kernel_proof! {
        if before_retry@.phase == Phase::AwaitingRob {
            handler_safety::continue_step(before_retry@, *m);
        }
    }
}
}
