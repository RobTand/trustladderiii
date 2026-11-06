//! Q54 / #440 / #806: shared real-transition fail-fast diversion contracts.
use super::*;

kernel_fn! {
/// Q54: the shared review stop has the exact production stopping update.
/// One transition, arbitrary node fields, and no bounded-world expansion.
pub(super) fn verify_stop(mut n: Node, step: ReviewStop) -> (result: Node) [
    requires n.generation < u64::MAX,
    ensures result == retirement::stopped(Node { review_stop: Some(step), ..n }, Outcome::Stopped),
] {
    stop_for_review(&mut n, step);
    n
}
}

kernel_fn! {
/// Q54 planner can't-meet: increment the local counter, then stop without
/// spending credit, emitting output, or allocating a question.
pub(super) fn verify_plan_stop(mut w: World, i: usize) -> (result: (World, Outbox)) [
    requires i < w.nodes@.len(), w.fail_fast,
        node_headroom(w.nodes@[i as int]), arithmetic_consistent(w.nodes@[i as int]),
        w.next_question < u64::MAX,
    ensures result.0.nodes@[i as int] == retirement::stopped(Node {
            review_stop: Some(ReviewStop::PlanCantMeet),
            plan_retries: if w.nodes@[i as int].plan_retries < 255 {
                (w.nodes@[i as int].plan_retries + 1) as u8
            } else { 255u8 },
            ..w.nodes@[i as int]
        }, Outcome::Stopped),
        result.0.next_question == w.next_question, result.1.len == 0,
] proof [reveal(empty_outbox);] {
    let mut out = Outbox::new();
    plan_retry(&mut w, i, true, &mut out);
    (w, out)
}
}
