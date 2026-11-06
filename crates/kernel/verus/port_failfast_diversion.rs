//! Q54 / #440 / #806: shared real-transition fail-fast diversion contracts.
use super::*;

verus! {
/// Q54: the existing planner counter saturates before choosing stop, retry, or question.
pub(super) open spec fn plan_count(n: Node) -> u8 {
    if n.plan_retries < 255 { (n.plan_retries + 1) as u8 } else { 255u8 }
}

/// Q54: exact review stop and the ordinary retry outcome asserted by the original paired check.
#[verifier::opaque]
pub(super) open spec fn plan_fields(before: Node, after: Node,
    fast: bool, cant: bool, id: u64) -> bool {
    if fast && cant {
        before.phase is Planning ==> after == retirement::stopped(Node { review_stop: Some(ReviewStop::PlanCantMeet),
            plan_retries: plan_count(before), ..before }, Outcome::Stopped)
    } else {
        let first = before.plan_retries == 0;
        after.phase == if first { before.phase } else { Phase::AwaitingRob }
            && after.plan_retries == if first { 1u8 } else { 0u8 }
            && after.question == if first { before.question } else {
                Some(Question { id, kind: QKind::CantMeet, cont: Cont::Planning {
                    must_split: matches!(before.phase, Phase::Planning { must_split: true }) },
                    cost: 1, clarified: false })
            }
    }
}

/// Q54: the shared planner stopping/retry contract includes output and question allocation.
#[verifier::opaque]
pub(super) open spec fn plan_result(before: Node, after: Node, fast: bool, cant_meet: bool,
    question: u64, next_question: u64, before_out: Outbox, after_out: Outbox) -> bool {
    plan_fields(before, after, fast, cant_meet, question)
        && if fast && cant_meet || before.plan_retries == 0 {
            next_question == question && after_out == before_out
        } else {
            next_question as int == question as int + 1 && after_out.len == before_out.len + 1
        }
}

/// Q54: forward the local planner result alongside the existing retry_bounds frame.
#[verifier::opaque]
pub(super) open spec fn plan_retry_post(before: World, after: World, i: int,
    cant_meet: bool, before_out: Outbox, after_out: Outbox) -> bool {
    plan_result(before.nodes@[i], after.nodes@[i], before.fail_fast, cant_meet,
        before.next_question, after.next_question, before_out, after_out)
}
}

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
    requires i < w.nodes@.len(), w.fail_fast, w.nodes@[i as int].phase is Planning,
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
] proof [reveal(empty_outbox); reveal(plan_retry_post); reveal(plan_result); reveal(plan_fields);] {
    let mut out = Outbox::new();
    plan_retry(&mut w, i, true, &mut out);
    (w, out)
}
}
