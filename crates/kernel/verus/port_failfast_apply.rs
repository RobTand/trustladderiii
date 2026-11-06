//! Q54 / #440 / #806: the paired original-domain diversion over real apply.
use super::*;

kernel_fn! {
/// Q54 PlanCantMeet: compare real apply with fail_fast off and on, for every
/// generation and both Planning variants. Preserve every original diverted
/// assertion and the complete fast-mode checker and shared node/output rules.
/// Settledness, four-file bounds and small ledgers are unnecessary.
pub(super) fn plan(mut off: World, generation: u64)
    -> (result: ((World, Outbox), (World, Outbox))) [
    requires retirement::domain(off), retirement::accepted(off), retirement::bounded(off),
        storage_ordered(off), !off.fail_fast, off.file_sets@.len() == 0,
        off.nodes@[0].phase is Planning,
    ensures !result.0.0.fail_fast, result.1.0.fail_fast,
        retirement::domain(result.1.0), retirement::accepted(result.1.0),
        retirement::notices_only(result.1.1), result.1.1.len == 0,
        forall|i: int| 0 <= i < 2 ==> port_failfast_safety::node_post(off.nodes@[i], result.1.0.nodes@[i]),
        !port_failfast_admission::same_world(result.0.0, result.1.0) ==> {
            result.1.0.nodes@[1] == result.0.0.nodes@[1]
                && result.1.0.nodes@[0].credit == off.nodes@[0].credit
                && result.1.0.nodes@[0].phase == Phase::Stopping(Outcome::Stopped)
                && result.1.0.nodes@[0].review_stop == Some(ReviewStop::PlanCantMeet)
                && port_failfast_pair::retried(off.nodes@[0], result.0.0.nodes@[0])
        },
        port_review_reply::active(off.nodes@[0], generation) ==>
            result.1.0.nodes@[0].phase == Phase::Stopping(Outcome::Stopped)
                && result.1.0.nodes@[0].review_stop == Some(ReviewStop::PlanCantMeet)
                && port_failfast_pair::retried(off.nodes@[0], result.0.0.nodes@[0]),
        port_review_reply::active(off.nodes@[0], generation) && off.nodes@[0].plan_retries == 0 ==>
            result.0.0.nodes@[0].phase == off.nodes@[0].phase,
        !port_review_reply::active(off.nodes@[0], generation) ==>
            port_failfast_admission::same_world(result.0.0, result.1.0)
                && result.0.0.nodes@[0] == off.nodes@[0] && result.0.1.len == 0,
] proof [reveal(retirement_domain); reveal(retirement_accepted); reveal(empty_outbox);
    reveal(retirement::notices_only); reveal(storage_ordered); reveal(footprint::ordered_table);] {
    kernel_snapshot! { before = off }
    let mut on = port_failfast_witness::fast_copy(&off);
    kernel_snapshot! { before_on = on }
    kernel_proof! {
        port_failfast_admission::stale_world(off, on);
        port_replies_safety::apply_scope(off);
        port_replies_safety::apply_scope(on);
    }
    let checked_before = off.check();
    assert!(checked_before.is_ok());
    let mut out_off = Outbox::new();
    let mut out_on = Outbox::new();
    kernel_snapshot! { initial_out = out_off }
    kernel_snapshot! { initial_on_out = out_on }
    apply(&mut off, Event::Reply { node: NodeId(0), generation, reply: Reply::PlanCantMeet }, &mut out_off);
    apply(&mut on, Event::Reply { node: NodeId(0), generation, reply: Reply::PlanCantMeet }, &mut out_on);
    kernel_proof! {
        if port_review_reply::active(before@.nodes@[0], generation) {
            port_failfast_plan::accepted_result(before@, off, generation, initial_out@, out_off);
            port_failfast_plan::accepted_result(before_on@, on, generation, initial_on_out@, out_on);
            port_failfast_pair_frame::compare(before@, off, on,
                port_review_reply::ended(before@.nodes@[0]), 2, ReviewStop::PlanCantMeet, out_on);
        } else {
            reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
            assert(off.nodes@ =~= on.nodes@);
        }
        if port_review_reply::active(before@.nodes@[0], generation) {
            port_failfast_plan::stopped_reply(before_on@, on, generation, initial_on_out@, out_on);
            port_failfast_plan::accepted_result(before@, off, generation, initial_out@, out_off);
            reveal(port_failfast_diversion::plan_fields);
        } else {
            reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
            assert(before_on@.nodes@ =~= on.nodes@);
            assert(port_failfast_admission::same_world(before_on@, on));
            port_failfast_admission::stale_world(before_on@, on);
        }
        assert forall|i: int| 0 <= i < 2 implies
            port_failfast_safety::node_post(before@.nodes@[i], on.nodes@[i]) by {
            assert(i == 0 || i == 1);
        }
    }
    kernel_proof! { port_replies_safety::checker_headroom(before_on@, on); }
    let checked_after = on.check();
    assert!(checked_after.is_ok());
    ( (off, out_off), (on, out_on) )
}
}

kernel_fn! {
/// Q54 / #440 original diverted cover: build a settled original-domain world
/// and call real apply twice. Both Planning variants reach the review diversion.
pub(super) fn plan_diverted(split: bool) -> (result: ((World, Outbox), (World, Outbox))) [
    ensures !port_failfast_admission::same_world(result.0.0, result.1.0),
        result.0.0.nodes@[0].phase == (Phase::Planning { must_split: split }),
        result.1.0.nodes@[0].phase == Phase::Stopping(Outcome::Stopped),
        result.1.0.nodes@[0].review_stop == Some(ReviewStop::PlanCantMeet),
        retirement::accepted(result.1.0), result.1.1.len == 0,
] {
    let w = port_failfast_witness::world(true, split, false);
    let result = plan(w, 0);
    result
}
}

kernel_fn! {
/// Q54 / #440 original unchanged cover: a stale generation in the same settled
/// original construction calls real apply twice and returns identical worlds
/// after normalizing only fail_fast, exactly the original unchanged cover.
pub(super) fn plan_unchanged(split: bool) -> (result: ((World, Outbox), (World, Outbox))) [
    ensures port_failfast_admission::same_world(result.0.0, result.1.0),
        result.1.0.nodes@[0].phase == (Phase::Planning { must_split: split }),
        result.0.1.len == 0, result.1.1.len == 0,
        retirement::accepted(result.1.0),
] {
    let w = port_failfast_witness::world(true, split, false);
    let result = plan(w, 1);
    result
}
}
