//! #806: node-only contracts for the four independently framed answers.
use super::port_answers_model::*;
use super::*;

verus! {
/// #806: no question, stale question, unrelated phase or unsupported offer stutters.
pub(super) proof fn post_unchanged(n: Node, id: u64, answer: Answer, next: u64)
    requires answers::answer_supported(answer),
        match n.question {
            None => true,
            Some(q) => q.id != id || match n.phase {
                Phase::Stopping(_) => answer != Answer::Retry,
                Phase::AwaitingRob => answer != Answer::Unclear
                    && (!offered(q.kind, answer) || answer == Answer::Retry),
                _ => true,
            },
        },
    ensures answers::answer_post(n, n, id, answer, next, next),
{
    reveal(answers::answer_post); reveal(answered);
}

/// #806: Retry clears only a current stopping question.
pub(super) proof fn post_retry(n: Node, id: u64, next: u64)
    requires n.phase is Stopping, n.question.is_some(), n.question.unwrap().id == id,
    ensures answers::answer_post(n, Node { question: None, ..n }, id, Answer::Retry, next, next),
{
    reveal(answers::answer_post); reveal(answered);
}

/// #806: one clarification changes only the question and its allocator.
pub(super) proof fn post_unclear(n: Node, id: u64, next: u64)
    requires n.phase == Phase::AwaitingRob, n.question.is_some(), n.question.unwrap().id == id,
    ensures answers::answer_post(n,
        if n.question.unwrap().clarified { n } else {
            Node { question: Some(Question { id: next, clarified: true, ..n.question.unwrap() }), ..n }
        }, id, Answer::Unclear, next, if n.question.unwrap().clarified { next } else { (next + 1) as u64 }),
{
    reveal(answers::answer_post); reveal(answered);
}

/// #806: Stop and ChangeRequest delegate to the existing exact stopping frame.
pub(super) proof fn post_stopping(n: Node, id: u64, answer: Answer, next: u64)
    requires n.phase == Phase::AwaitingRob, n.question.is_some(), n.question.unwrap().id == id,
        matches!(answer, Answer::Stop | Answer::ChangeRequest), offered(n.question.unwrap().kind, answer),
    ensures answers::answer_post(n,
        retirement::stopped(n, if answer == Answer::Stop { Outcome::Stopped } else { Outcome::Replaced }),
        id, answer, next, next),
{
    reveal(answers::answer_post); reveal(answered);
}

/// #805 / #806: cleanup-answer eligibility gives a teardown item, without a world in the handler.
pub(super) proof fn cleanup_input(w: World, n: Node, id: u64, answer: Answer)
    ensures node_checked(w, n) && answer_progress_input(n, id, answer) && n.phase is Stopping
        ==> answer == Answer::MarkHandled && n.effect.is_none() && !n.waiting_incident
            && matches!(current_item(Node { question: None, ..n }), Some(Item::Teardown(_))),
{}

/// #805 / #806: clearing a stopping question removes four units and preserves cleanup shape.
pub(super) proof fn stopping_question(n: Node)
    requires n.phase is Stopping, n.question.is_some(),
    ensures settle_shape(Node { question: None, ..n }) == settle_shape(n),
        n.effect.is_none() && !n.waiting_incident
            ==> distance(Node { question: None, ..n }) + 4 == distance(n),
{ reveal(distance); }
}
