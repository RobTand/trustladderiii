//! Q54 / #440 / #806: shared real-transition fail-fast diversion contracts.
use super::*;

verus! {
/// Q54: the existing planner counter saturates before choosing stop, retry, or question.
pub(super) open spec fn plan_count(n: Node) -> u8 {
    if n.plan_retries < 255 { (n.plan_retries + 1) as u8 } else { 255u8 }
}

/// Q54: exact local update of the real planner retry, for both flag values.
#[verifier::opaque]
pub(super) open spec fn plan_node(n: Node, fail_fast: bool, cant_meet: bool, question: u64) -> Node {
    let counted = Node { plan_retries: plan_count(n), ..n };
    if fail_fast && cant_meet {
        retirement::stopped(Node { review_stop: Some(ReviewStop::PlanCantMeet), ..counted }, Outcome::Stopped)
    } else if counted.plan_retries < 2 {
        retirement::phased(counted, n.phase)
    } else {
        retirement::phased(Node { plan_retries: 0, holds: false,
            question: Some(Question { id: question, kind: QKind::CantMeet,
                cont: Cont::Planning { must_split: matches!(n.phase, Phase::Planning { must_split: true }) },
                cost: 1, clarified: false }), ..counted }, Phase::AwaitingRob)
    }
}

/// Q54 planner stop: compose the counted node with the exact stop contract.
pub(super) proof fn plan_stop_result(before: Node, after: Node, fast: bool, cant: bool, id: u64)
    requires fast && cant,
        after == retirement::stopped(Node { review_stop: Some(ReviewStop::PlanCantMeet),
            plan_retries: plan_count(before), ..before }, Outcome::Stopped),
    ensures after == plan_node(before, fast, cant, id),
{
    reveal(plan_node);
}

/// Q54 planner first retry: compose the counted node with the exact phase contract.
pub(super) proof fn plan_rerun_result(before: Node, after: Node, fast: bool, cant: bool, id: u64)
    requires !(fast && cant), before.plan_retries == 0,
        after == retirement::phased(Node { plan_retries: 1, ..before }, before.phase),
    ensures after == plan_node(before, fast, cant, id),
{
    reveal(plan_node);
}

/// Q54 planner exhausted retry: compose the exact existing question allocation.
pub(super) proof fn plan_ask_result(before: Node, after: Node, fast: bool, cant: bool, id: u64)
    requires !(fast && cant), before.plan_retries > 0,
        after == retirement::phased(Node { plan_retries: 0, holds: false,
            question: Some(Question { id, kind: QKind::CantMeet,
                cont: Cont::Planning { must_split: matches!(before.phase, Phase::Planning { must_split: true }) },
                cost: 1, clarified: false }), ..before }, Phase::AwaitingRob),
    ensures after == plan_node(before, fast, cant, id),
{
    reveal(plan_node);
}

/// Q54: exact local planner result, kept opaque across world-frame composition.
#[verifier::opaque]
pub(super) open spec fn plan_result(before: Node, after: Node, fast: bool, cant_meet: bool,
    question: u64, next_question: u64, before_out: Outbox, after_out: Outbox) -> bool {
    after == plan_node(before, fast, cant_meet, question)
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

/// Q54: derive the exact local retry result without importing a whole-world frame.
pub(super) proof fn compose_plan(before: Node, after: Node, fast: bool, cant_meet: bool,
    question: u64, next_question: u64, before_out: Outbox, after_out: Outbox)
    requires if fast && cant_meet {
        after == retirement::stopped(Node { review_stop: Some(ReviewStop::PlanCantMeet),
            plan_retries: plan_count(before), ..before }, Outcome::Stopped)
            && next_question == question && after_out == before_out
    } else if before.plan_retries == 0 {
        after == retirement::phased(Node { plan_retries: 1, ..before }, before.phase)
            && next_question == question && after_out == before_out
    } else {
        after == retirement::phased(Node { plan_retries: 0, holds: false,
            question: Some(Question { id: question, kind: QKind::CantMeet,
                cont: Cont::Planning { must_split: matches!(before.phase, Phase::Planning { must_split: true }) },
                cost: 1, clarified: false }), ..before }, Phase::AwaitingRob)
            && next_question as int == question as int + 1 && after_out.len == before_out.len + 1
    },
    ensures plan_result(before, after, fast, cant_meet, question, next_question, before_out, after_out),
{
    reveal(plan_result);
    reveal(plan_node);
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
] proof [reveal(empty_outbox); reveal(plan_retry_post); reveal(plan_result); reveal(plan_node);] {
    let mut out = Outbox::new();
    plan_retry(&mut w, i, true, &mut out);
    (w, out)
}
}
