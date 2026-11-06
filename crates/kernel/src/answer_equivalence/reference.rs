//! #806: verbatim pre-extraction on_answer body, reference head 3fb66a5cb2.
use crate::*;

pub(super) fn on_answer(w: &mut World, i: usize, id: u64, answer: Answer, out: &mut Outbox) {
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
