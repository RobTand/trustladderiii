//! Q54 / #440 / #806: consume the real shared retry helpers before reply forwarding.
use super::*;

kernel_fn! {
/// Q54 maker/planner rows: one caller bridges the real ladder, exhaustion and
/// planner helpers to the parametric diversion relation and unchanged-world frame.
pub(super) fn run(mut w: World, kind: u8, step: ReviewStop) -> (result: (World, Outbox)) [
    requires w.nodes@.len() == 2, kind < 3,
        node_headroom(w.nodes@[0]), arithmetic_consistent(w.nodes@[0]),
        w.next_question < u64::MAX,
        kind == 2 ==> w.nodes@[0].phase is Planning && step == ReviewStop::PlanCantMeet,
    ensures port_failfast_pair::retry_post(w.nodes@[0],
            (result.0.nodes@[0], result.0.next_question), w.fail_fast, w.next_question, kind, step),
        port_failfast_pair_frame::frame(w, result.0), result.0.fail_fast == w.fail_fast,
        retry_bounds(w, result.0, 0, 0, result.1.len),
        w.fail_fast ==> result.1.len == 0,
] proof [reveal(empty_outbox); reveal(retry_bounds);
    reveal(port_failfast_diversion::plan_retry_post); reveal(port_failfast_diversion::plan_result);] {
    let mut out = Outbox::new();
    if kind == 0 {
        next_rung(&mut w, 0, step, &mut out);
    } else if kind == 1 {
        out_of_room(&mut w, 0, step, &mut out);
    } else {
        plan_retry(&mut w, 0, true, &mut out);
    }
    (w, out)
}
}

verus! {
/// Q54: normalize the actual helper's fast result locally, before composing
/// the opaque complete-world checker frame.
pub(super) proof fn stop_node(source: Node, after: Node, id: u64, next: u64,
    kind: u8, step: ReviewStop)
    requires kind < 3, !(source.phase is Stopping || source.phase is Closed),
        kind == 2 ==> source.phase is Planning && step == ReviewStop::PlanCantMeet,
        port_failfast_pair::retry_post(source, (after, next), true, id, kind, step),
    ensures after == retirement::stopped(Node { review_stop: Some(step),
            plan_retries: after.plan_retries, ..source }, Outcome::Stopped),
        next == id,
{
    reveal(port_review_retry::room);
    reveal(port_failfast_diversion::plan_fields);
    reveal(retirement::stopped);
}
}

kernel_fn! {
/// Q54 / #440: real operation completion followed by a real fail-fast helper
/// preserves the complete checker and every shared node assertion.
/// This is helper composition, pending the owners' real-apply forwarding.
pub(super) fn completed_stop(mut w: World, kind: u8, step: ReviewStop)
    -> (result: (World, Outbox)) [
    requires retirement::domain(w), retirement::accepted(w), w.fail_fast, kind < 3,
        node_headroom(w.nodes@[0]), arithmetic_consistent(w.nodes@[0]),
        w.next_question < u64::MAX,
        !(w.nodes@[0].phase is Stopping || w.nodes@[0].phase is Closed),
        kind == 2 ==> w.nodes@[0].phase is Planning && step == ReviewStop::PlanCantMeet,
    ensures retirement::domain(result.0), retirement::accepted(result.0),
        port_failfast_safety::node_post(w.nodes@[0], result.0.nodes@[0]),
        result.0.nodes@[1] == w.nodes@[1], result.1.len == 0,
        result.0.nodes@[0].phase == Phase::Stopping(Outcome::Stopped),
        result.0.nodes@[0].review_stop == Some(step),
        result.0.nodes@[0].credit == w.nodes@[0].credit,
        result.0.nodes@[0].question.is_none(), result.0.next_question == w.next_question,
] {
    kernel_snapshot! { before = w }
    end_operation(&mut w.nodes[0]);
    kernel_snapshot! { source = w }
    let result = run(w, kind, step);
    kernel_proof! {
        stop_node(source@.nodes@[0], result.0.nodes@[0], source@.next_question,
            result.0.next_question, kind, step);
        port_failfast_frame::completed_stop(before@, result.0, step);
    }
    result
}
}

kernel_fn! {
/// Q54 / #440: compare actual shared-helper results under both flag values,
/// after the real end_operation. One parametric comparison covers all slice-1 retries.
pub(super) fn pair(mut w: World, kind: u8, step: ReviewStop)
    -> (result: ((World, Outbox), (World, Outbox))) [
    requires w.nodes@.len() == 2, !w.fail_fast, kind < 3,
        w.file_sets@.len() == 0, w.dependencies@.len() == 0,
        node_headroom(w.nodes@[0]), arithmetic_consistent(w.nodes@[0]),
        w.next_question < u64::MAX,
        !(w.nodes@[0].phase is Stopping || w.nodes@[0].phase is Closed),
        kind == 2 ==> w.nodes@[0].phase is Planning && step == ReviewStop::PlanCantMeet,
    ensures !result.0.0.fail_fast, result.1.0.fail_fast, result.1.1.len == 0,
        !port_failfast_admission::same_world(result.0.0, result.1.0) ==> {
            result.1.0.nodes@[1] == result.0.0.nodes@[1]
                && result.1.0.nodes@[0].credit == w.nodes@[0].credit
                && result.1.0.nodes@[0].phase == Phase::Stopping(Outcome::Stopped)
                && result.1.0.nodes@[0].review_stop == Some(step)
                && port_failfast_pair::retried(w.nodes@[0], result.0.0.nodes@[0])
        },
] {
    end_operation(&mut w.nodes[0]);
    kernel_snapshot! { source = w }
    let on = port_failfast_witness::fast_copy(&w);
    let off_result = run(w, kind, step);
    let on_result = run(on, kind, step);
    kernel_proof! {
        port_failfast_pair_frame::compare(source@, off_result.0, on_result.0,
            source@.nodes@[0], kind, step, on_result.1);
    }
    (off_result, on_result)
}
}
