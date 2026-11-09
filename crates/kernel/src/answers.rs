//! Executable command safety contracts, split to bound each required query.
use super::*;

kernel_fn! {
/// Question answer rows / #771: one clarification or one paid continuation.
/// K5 value form of on_answer's work: the node and next question id come in by
/// value; the answered node, the next id and at most one notice are returned.
pub(super) fn answer_node(n: Node, nq: u64, id: u64, answer: Answer)
    -> (result: (Node, u64, Option<Notice>)) [
    requires node_headroom(n),
        arithmetic_consistent(n),
        nq < u64::MAX,
    ensures (handler_safety::node_ok(n, nq)
            && answer_progress_input(n, id, answer)
            ==> distance(result.0) < distance(n)),
        (n.phase is Closed ==> result.0.phase is Closed),
        result.0.repo == n.repo,
        result.0.footprint == n.footprint,
        (settle_shape(n) ==> settle_shape(result.0)),
        result.0.generation as int <= n.generation as int + 1,
        result.0.next_seq == n.next_seq,
        result.0.credit.start == n.credit.start,
        result.0.credit.earned == n.credit.earned,
        result.0.credit.added as int <= n.credit.added as int + 255,
        result.0.credit.spent as int <= n.credit.spent as int + 1,
        result.0.last_cmd == n.last_cmd,
        result.1 as int <= nq as int + 1,
        result.0.proposed == n.proposed,
        result.0.adopted == n.adopted,
        (n.phase is Closed ==> result.0.phase == n.phase),
        handler_safety::command_step(n, result.0),
        (handler_safety::node_ok(n, nq) && result.0 != n ==> (match n.question {
            Some(q) => q.id == id,
            None => false,
        }) && (match result.0.question {
            Some(q) => q.id != id,
            None => true,
        })),
] proof [reveal(distance);] {
    let mut m = n;
    let mut nq = nq;
    let mut note: Option<Notice> = None;
    let Some(q) = m.question else {
        kernel_proof! { handler_safety::same_step(n); }
        return (m, nq, note);
    };
    if q.id != id {
        kernel_proof! { handler_safety::same_step(n); }
        return (m, nq, note); // an answer to a question that is no longer open
    }
    match m.phase {
        Phase::Stopping(_) => match answer {
            Answer::Retry => {
                m.question = None;
                kernel_proof! { handler_safety::field_step(n, m); }
            }
            Answer::MarkHandled => {
                m.question = None;
                mark_handled(&mut m);
                kernel_proof! { handler_safety::field_step(n, m); }
            }
            _ => { kernel_proof! { handler_safety::same_step(n); } }
        },
        Phase::AwaitingRob => {
            if answer == Answer::Unclear {
                // One clarifying question. After that the question just stays open.
                if !q.clarified {
                    let (m1, nq1, x) = clarify_node(m, nq, q);
                    m = m1;
                    nq = nq1;
                    note = Some(x);
                }
                kernel_proof! { if q.clarified { handler_safety::same_step(n); } }
                return (m, nq, note);
            }
            if !allowed(q.kind, answer) {
                kernel_proof! { handler_safety::same_step(n); }
                return (m, nq, note);
            }
            match answer {
                Answer::Continue => {
                    let (m1, x) = go_on(m, q, 0);
                    m = m1;
                    note = x;
                }
                Answer::TryAgain => {
                    let (m1, x) = retry_answer_node(m, q);
                    m = m1;
                    note = x;
                }
                Answer::AddCredit(k) => {
                    let (m1, x) = go_on(m, q, k);
                    m = m1;
                    note = x;
                }
                Answer::ChangeRequest => {
                    kernel_snapshot! { before_replace = m }
                    m = enter_stopping_v(m, Outcome::Replaced);
                    kernel_proof! {
                        assert(m.proposed == before_replace@.proposed && m.adopted == before_replace@.adopted)
                            by { reveal(retirement::stopped); }
                        handler_safety::stop_step(before_replace@, Outcome::Replaced);
                    }
                }
                Answer::Stop => {
                    kernel_snapshot! { before_stop = m }
                    m = enter_stopping_v(m, Outcome::Stopped);
                    kernel_proof! {
                        assert(m.proposed == before_stop@.proposed && m.adopted == before_stop@.adopted)
                            by { reveal(retirement::stopped); }
                        handler_safety::stop_step(before_stop@, Outcome::Stopped);
                    }
                }
                _ => { kernel_proof! { handler_safety::same_step(n); } }
            }
        }
        _ => { kernel_proof! { handler_safety::same_step(n); } }
    }
    (m, nq, note)
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
/// Clarification rows / #771: one clarifying question under a fresh id, as a
/// value function (K5): the node, the next id and the Ask notice are returned.
fn clarify_node(n: Node, nq: u64, q: Question) -> (result: (Node, u64, Notice)) [
    requires nq < u64::MAX,
    ensures result.0 == (Node {
            question: Some(Question { id: nq, clarified: true, ..q }), ..n
        }),
        result.1 == nq + 1,
        result.2 == Notice::Ask { question: nq },
        n.phase == Phase::AwaitingRob && n.question == Some(q) && !q.clarified
            ==> handler_safety::command_step(n, result.0),
] {
    let mut m = n;
    let next = nq;
    m.question = Some(Question {
        id: next,
        clarified: true,
        ..q
    });
    kernel_proof! {
        if n.phase == Phase::AwaitingRob && n.question == Some(q) && !q.clarified {
            handler_safety::clarify_step(n, q, next);
        }
    }
    (m, next + 1, Notice::Ask { question: next })
}
}

kernel_fn! {
/// TryAgain rows / #771: restart the first attempt, then continue with at least
/// one credit, as a value function (K5); the continuation is continue_node, then
/// go_on's replanned note (never due here: replanned was just cleared).
fn retry_answer_node(n: Node, q: Question) -> (result: (Node, Option<Notice>)) [
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> (if q.cost > 1 { q.cost } else { 1u8 }) == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures ({
            let b = try_again_node(n);
            let q1 = Question { cost: if q.cost > 1 { q.cost } else { 1u8 }, ..q };
            (b.phase == Phase::AwaitingRob && b.question == Some(q1) && !b.paused
                && !b.waiting_incident && b.effect.is_none()
                ==> distance(result.0) < distance(b))
            && result.0.repo == b.repo && result.0.footprint == b.footprint
            && (settle_shape(b) ==> settle_shape(result.0))
            && result.0.generation as int <= b.generation as int + 1
            && result.0.next_seq == b.next_seq
            && result.0.credit.start == b.credit.start && result.0.credit.earned == b.credit.earned
            && result.0.credit.added as int <= b.credit.added as int + 255
            && result.0.credit.spent as int <= b.credit.spent as int + 1
            && result.0.last_cmd == b.last_cmd
            && result.0.question.is_none()
            && result.0.proposed == b.proposed && result.0.adopted == b.adopted
            && result.0.review_stop == b.review_stop && result.0.attempt == 1
            && !result.0.replanned
        }),
        n.phase == Phase::AwaitingRob ==> handler_safety::command_step(n, result.0),
        result.1.is_none(),
] {
    let mut m = n;
    m.attempt = 1;
    m.review_restarted = false;
    m.round = 0;
    m.replanned = false;
    m = continue_node(
        m,
        Question {
            cost: q.cost.max(1),
            ..q
        },
        0,
    );
    let mut note: Option<Notice> = None;
    match q.cont {
        Cont::Planning { must_split } => {
            if !must_split && m.replanned && q.kind == QKind::OutOfCredit {
                note = Some(Notice::Note(NoteKind::Replanned));
            }
        }
        _ => {}
    }
    kernel_proof! {
        if n.phase == Phase::AwaitingRob {
            handler_safety::continue_step(n, m);
        }
    }
    (m, note)
}
}
