//! Q54 / #440 / #806: consume W1's planner forwarding at the accepted worker seam.
use super::*;

verus! {
/// Q54 PlanCantMeet: the accepted worker relation supplies the shared retry
/// result, unchanged file storage, and the exact fast output length.
pub(super) proof fn worker_result(before: World, after: World, source: Node,
    initial_out: Outbox, out: Outbox)
    requires before.dependencies@.len() == 0, after.nodes@.len() == 2,
        source.phase is Planning, before.next_question < u64::MAX,
        port_replies_contracts::transition_post(before, after, 0, source,
            Reply::PlanCantMeet, initial_out, out),
    ensures port_failfast_pair::retry_post(source, (after.nodes@[0], after.next_question),
            before.fail_fast, before.next_question, 2, ReviewStop::PlanCantMeet),
        after.file_sets@ == before.file_sets@,
        before.fail_fast ==> out.len == initial_out.len,
        retirement::notices_only(initial_out) ==> retirement::notices_only(out),
{
    reveal(port_replies_contracts::transition_post);
    reveal(port_failfast_diversion::plan_result);
}

/// Q54 PlanCantMeet: unwrap accepted admission once, retaining the actual
/// post-end_operation source required by W1's planner relation.
pub(super) proof fn accepted_result(before: World, after: World, generation: u64,
    initial_out: Outbox, out: Outbox)
    requires retirement::domain(before), before.nodes@[0].phase is Planning,
        before.next_question < u64::MAX,
        port_review_reply::active(before.nodes@[0], generation),
        port_review_reply::reply_post(before, after, 0, generation,
            Reply::PlanCantMeet, initial_out, out),
    ensures port_failfast_pair_frame::frame(before, after), after.fail_fast == before.fail_fast,
        port_failfast_pair::retry_post(port_review_reply::ended(before.nodes@[0]),
            (after.nodes@[0], after.next_question), before.fail_fast,
            before.next_question, 2, ReviewStop::PlanCantMeet),
        before.fail_fast ==> out.len == initial_out.len,
        retirement::notices_only(initial_out) ==> retirement::notices_only(out),
{
    reveal(port_review_reply::reply_post);
    reveal(port_review_reply::reply_result); reveal(port_review_reply::accepted_reply);
    reveal(port_review_reply::transition_post);
    worker_result(before, after, port_review_reply::ended(before.nodes@[0]), initial_out, out);
}

/// Q54 / #440 PlanCantMeet: compose the accepted planner stop with the complete
/// checker and every shared node assertion, once generic reply forwarding arrives.
pub(super) proof fn stopped_reply(before: World, after: World, generation: u64,
    initial_out: Outbox, out: Outbox)
    requires retirement::domain(before), retirement::accepted(before), before.fail_fast,
        before.nodes@[0].phase is Planning, node_headroom(before.nodes@[0]),
        before.next_question < u64::MAX,
        port_review_reply::active(before.nodes@[0], generation),
        port_review_reply::reply_post(before, after, 0, generation,
            Reply::PlanCantMeet, initial_out, out),
    ensures retirement::domain(after), retirement::accepted(after),
        port_failfast_safety::node_post(before.nodes@[0], after.nodes@[0]),
        after.nodes@[1] == before.nodes@[1], after.next_question == before.next_question,
        after.nodes@[0].phase == Phase::Stopping(Outcome::Stopped),
        after.nodes@[0].review_stop == Some(ReviewStop::PlanCantMeet),
        after.nodes@[0].credit == before.nodes@[0].credit,
        after.nodes@[0].question.is_none(), out.len == initial_out.len,
{
    accepted_result(before, after, generation, initial_out, out);
    completed_result(before, after);
}

/// Q54 / #440: isolate the accepted planner's local stop normalization from
/// admission and its output relation before composing the complete world frame.
pub(super) proof fn completed_result(before: World, after: World)
    requires retirement::domain(before), retirement::accepted(before),
        before.nodes@[0].phase is Planning, node_headroom(before.nodes@[0]),
        port_failfast_pair_frame::frame(before, after),
        port_failfast_pair::retry_post(port_review_reply::ended(before.nodes@[0]),
            (after.nodes@[0], after.next_question), true, before.next_question, 2, ReviewStop::PlanCantMeet),
    ensures retirement::domain(after), retirement::accepted(after),
        port_failfast_safety::node_post(before.nodes@[0], after.nodes@[0]),
        after.nodes@[0].credit == before.nodes@[0].credit,
        after.nodes@[0].question.is_none(), after.next_question == before.next_question,
        after.nodes@[0].phase == Phase::Stopping(Outcome::Stopped),
        after.nodes@[0].review_stop == Some(ReviewStop::PlanCantMeet),
{
    port_failfast_retry::stop_node(port_review_reply::ended(before.nodes@[0]),
        after.nodes@[0], before.next_question, after.next_question, 2, ReviewStop::PlanCantMeet);
    port_failfast_frame::completed_stop(before, after, ReviewStop::PlanCantMeet);
}

/// Q54 / #440 PlanCantMeet: the universal paired diversion, including arbitrary
/// stale generations. Its remaining caller premise is W2's real-apply reply_post.
pub(super) proof fn paired_reply(before: World, off: World, on: World, generation: u64,
    initial_out: Outbox, off_out: Outbox, on_out: Outbox)
    requires retirement::domain(before), !before.fail_fast,
        before.nodes@[0].phase is Planning, node_headroom(before.nodes@[0]),
        before.next_question < u64::MAX, initial_out.len == 0,
        port_review_reply::reply_post(before, off, 0, generation,
            Reply::PlanCantMeet, initial_out, off_out),
        port_review_reply::reply_post(World { fail_fast: true, ..before }, on,
            0, generation, Reply::PlanCantMeet, initial_out, on_out),
    ensures !port_failfast_admission::same_world(off, on) ==> {
        on.nodes@[1] == off.nodes@[1] && on.nodes@[0].credit == before.nodes@[0].credit
            && on.nodes@[0].phase == Phase::Stopping(Outcome::Stopped)
            && on.nodes@[0].review_stop == Some(ReviewStop::PlanCantMeet) && on_out.len == 0
            && port_failfast_pair::retried(before.nodes@[0], off.nodes@[0])
    },
{
    if port_review_reply::active(before.nodes@[0], generation) {
        accepted_result(before, off, generation, initial_out, off_out);
        accepted_result(World { fail_fast: true, ..before }, on, generation, initial_out, on_out);
        port_failfast_pair_frame::compare(before, off, on,
            port_review_reply::ended(before.nodes@[0]), 2, ReviewStop::PlanCantMeet, on_out);
    } else {
        port_failfast_admission::inactive_pair(before, off, on, generation,
            Reply::PlanCantMeet, initial_out, off_out, on_out);
    }
}
}

kernel_fn! {
/// Q54 PlanCantMeet: run the actual accepted dispatcher after real operation
/// completion. Generic on_reply/apply forwarding remains a separate owner seam.
pub(super) fn dispatch(mut w: World) -> (result: (World, Outbox)) [
    requires retirement::domain(w), retirement::accepted(w), w.nodes@[0].phase is Planning,
        node_headroom(w.nodes@[0]), arithmetic_consistent(w.nodes@[0]),
        w.next_question < u64::MAX,
    ensures port_failfast_pair_frame::frame(w, result.0), result.0.fail_fast == w.fail_fast,
        port_failfast_pair::retry_post(port_review_reply::ended(w.nodes@[0]),
            (result.0.nodes@[0], result.0.next_question), w.fail_fast,
            w.next_question, 2, ReviewStop::PlanCantMeet),
        retirement::notices_only(result.1),
        w.fail_fast ==> retirement::domain(result.0) && retirement::accepted(result.0)
            && port_failfast_safety::node_post(w.nodes@[0], result.0.nodes@[0])
            && result.1.len == 0 && result.0.next_question == w.next_question,
] proof [reveal(reply_bounds); reveal(empty_outbox); reveal(retirement::notices_only);] {
    kernel_snapshot! { before = w }
    end_operation(&mut w.nodes[0]);
    kernel_snapshot! { source = w }
    let mut out = Outbox::new();
    kernel_snapshot! { initial_out = out }
    reply_transition(&mut w, 0, Reply::PlanCantMeet, &mut out);
    kernel_proof! {
        worker_result(source@, w, source@.nodes@[0], initial_out@, out);
        if before@.fail_fast {
            port_failfast_retry::stop_node(source@.nodes@[0], w.nodes@[0],
                source@.next_question, w.next_question, 2, ReviewStop::PlanCantMeet);
            port_failfast_frame::completed_stop(before@, w, ReviewStop::PlanCantMeet);
        }
    }
    (w, out)
}
}

kernel_fn! {
/// Q54 / #440 PlanCantMeet: compare both actual accepted dispatches in the
/// original world construction. This supplies no real-apply cover claim.
pub(super) fn dispatch_pair(w: World) -> (result: ((World, Outbox), (World, Outbox))) [
    requires retirement::domain(w), retirement::accepted(w), !w.fail_fast,
        w.file_sets@.len() == 0, w.nodes@[0].phase is Planning,
        node_headroom(w.nodes@[0]), arithmetic_consistent(w.nodes@[0]),
        w.next_question < u64::MAX,
    ensures !result.0.0.fail_fast, result.1.0.fail_fast, result.1.1.len == 0,
        retirement::domain(result.1.0), retirement::accepted(result.1.0),
        !port_failfast_admission::same_world(result.0.0, result.1.0) ==> {
            result.1.0.nodes@[1] == result.0.0.nodes@[1]
                && result.1.0.nodes@[0].credit == w.nodes@[0].credit
                && result.1.0.nodes@[0].phase == Phase::Stopping(Outcome::Stopped)
                && result.1.0.nodes@[0].review_stop == Some(ReviewStop::PlanCantMeet)
                && port_failfast_pair::retried(w.nodes@[0], result.0.0.nodes@[0])
        },
] {
    kernel_snapshot! { before = w }
    let on = port_failfast_witness::fast_copy(&w);
    kernel_proof! { port_failfast_admission::stale_world(w, on); }
    let off_result = dispatch(w);
    let on_result = dispatch(on);
    kernel_proof! {
        port_failfast_pair_frame::compare(before@, off_result.0, on_result.0,
            port_review_reply::ended(before@.nodes@[0]), 2, ReviewStop::PlanCantMeet, on_result.1);
    }
    (off_result, on_result)
}
}
