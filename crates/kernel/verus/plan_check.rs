//! #832 Planning/Q54: source-connected safety and reachable semantic endings.
use super::*;

kernel_fn! {
/// #832: construct a Planning await and execute the real reply handler.
/// One node, one gate and counters below 8 witness both flags and retry counts.
/// The handler's quantified postcondition itself has no such storage bound.
fn check_plan_ending(fail_fast: bool, retries: u8, needs_split: bool) -> (result: ()) [
    requires retries <= 1,
] proof [reveal(initial_node); reveal(node_headroom); reveal(arithmetic_consistent);
    reveal(footprint::ordered_table);] {
    let mut n = Node::new(RepoId(0));
    n.phase = Phase::Planning { must_split: false };
    n.pos = 1;
    n.credit.ready = true;
    n.credit.start = 4;
    n.run = Some(0);
    n.pool = true;
    n.plan_retries = retries;
    let mut nodes = Vec::new();
    nodes.push(n);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    let mut w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 7,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast };
    let mut out = Outbox::new();
    kernel_proof! { footprint::inline_is_valid(w, 0); }
    let reply = if needs_split { Reply::PlanCheckNeedsSplit } else { Reply::PlanCheckRejected };
    on_reply(&mut w, 0, 0, reply, &mut out);
    kernel_proof! {
        assert(w.nodes@[0].phase == Phase::Stopping(Outcome::Stopped));
        assert(w.nodes@[0].question.is_none());
        assert(w.nodes@[0].review_stop == Some(ReviewStop::PlanCantMeet));
        assert(w.nodes@[0].credit == n.credit);
        assert(w.nodes@[0].plan_retries == retries);
        assert(w.next_question == 7);
        assert(empty_outbox(out));
    }
}
}

kernel_fn! {
/// #832 non-size/mixed objections: reachable under both fail_fast settings.
fn apply_plan_check_rejected() -> (result: ()) [] {
    check_plan_ending(false, 0, false);
    check_plan_ending(true, 1, false);
}
}

kernel_fn! {
/// #832 size/execution fit: the interim recorded-split stop asks nothing.
fn apply_plan_check_needs_split() -> (result: ()) [] {
    check_plan_ending(false, 1, true);
    check_plan_ending(true, 0, true);
}
}
