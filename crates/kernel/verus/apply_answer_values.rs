//! Exact opaque leaves for the Apply answer pilot.
use super::*;
verus! {
pub(super) open spec fn permits(kind: QKind, answer: Answer) -> bool {
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
pub(super) open spec fn replanned_note(n: Node, q: Question) -> Option<Notice> {
    if (match q.cont { Cont::Planning { must_split } => !must_split, _ => false })
        && n.replanned && q.kind == QKind::OutOfCredit {
        Some(Notice::Note(NoteKind::Replanned))
    } else { None }
}
#[verifier::opaque]
pub(super) open spec fn continued_answer(n: Node, q: Question, deposit: u8) -> (Node, Option<Notice>) {
    let m = handler_safety::continued_node(n, q, deposit);
    (m, replanned_note(m, q))
}
#[verifier::opaque]
pub(super) open spec fn retried_answer(n: Node, q: Question) -> (Node, Option<Notice>) {
    continued_answer(answers::try_again_node(n), Question { cost: if q.cost > 1 { q.cost } else { 1u8 }, ..q }, 0)
}
#[verifier::opaque]
pub(super) open spec fn answered(n: Node, nq: u64, id: u64, answer: Answer) -> (Node, u64, Option<Notice>) {
    let unchanged = (n, nq, None);
    match n.question {
        None => unchanged,
        Some(q) => if q.id != id { unchanged } else { match n.phase {
            Phase::Stopping(_) => match answer {
                Answer::Retry => (Node { question: None, ..n }, nq, None),
                Answer::MarkHandled => (handler_safety::handled_node(Node { question: None, ..n }), nq, None),
                _ => unchanged,
            },
            Phase::AwaitingRob => if answer == Answer::Unclear {
                if q.clarified { unchanged } else {
                    (Node { question: Some(Question { id: nq, clarified: true, ..q }), ..n },
                        (nq + 1) as u64, Some(Notice::Ask { question: nq }))
                }
            } else if !permits(q.kind, answer) { unchanged } else { match answer {
                Answer::Continue => { let r = continued_answer(n, q, 0); (r.0, nq, r.1) },
                Answer::TryAgain => { let r = retried_answer(n, q); (r.0, nq, r.1) },
                Answer::AddCredit(k) => { let r = continued_answer(n, q, k); (r.0, nq, r.1) },
                Answer::ChangeRequest => (retirement::stopped(n, Outcome::Replaced), nq, None),
                Answer::Stop => (retirement::stopped(n, Outcome::Stopped), nq, None),
                _ => unchanged,
            }},
            _ => unchanged,
        }},
    }
}
}
