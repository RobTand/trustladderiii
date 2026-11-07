//! Reachability for the real Q27 empty-allowed transition, both fail_fast values.
use super::*;

kernel_fn! {
/// The first reply starts a fresh generation. A second reply stops, with the
/// same credit ledger, no question, and no second re-plan. Storage is one node;
/// reply_empty_replan's quantified contract has no node-count bound.
fn check_empty_allowed_replan(fail_fast: bool) -> (result: ()) []
proof [reveal(initial_node); reveal(node_headroom); reveal(arithmetic_consistent);
    reveal(reply_bounds); reveal(retirement::phased);] {
    let mut n = Node::new(RepoId(0));
    n.phase = Phase::Planning { must_split: false };
    n.credit.ready = true;
    n.credit.start = 4;
    let mut nodes = Vec::new();
    nodes.push(n);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    let mut w = World { nodes, gates, pool_capacity: 1, next_ticket: 0,
        next_question: 7, file_sets: Vec::new(), dependencies: Vec::new(), fail_fast };
    let mut out = Outbox::new();
    let mut budget = ReplanBudget::new(NodeId(0));
    empty_replan::reply_empty_replan(&mut w, 0, &mut budget, &mut out);
    let consumed = budget.used();
    kernel_proof! {
        assert(w.nodes@[0].phase == Phase::Planning { must_split: false });
        assert(w.nodes@[0].generation == 1);
        assert(consumed);
        assert(w.nodes@[0].credit == n.credit);
        assert(w.nodes@[0].question.is_none());
        assert(w.next_question == 7);
        assert(empty_outbox(out));
    }
    empty_replan::reply_empty_replan(&mut w, 0, &mut budget, &mut out);
    let consumed = budget.used();
    kernel_proof! {
        assert(w.nodes@[0].phase == Phase::Stopping(Outcome::Stopped));
        assert(consumed);
        assert(w.nodes@[0].credit == n.credit);
        assert(w.nodes@[0].question.is_none());
        assert(w.next_question == 7);
        assert(empty_outbox(out));
    }
}
}

kernel_fn! {
/// Q27 reachability includes both fail_fast settings.
fn apply_empty_allowed_replan() -> (result: ()) [] {
    check_empty_allowed_replan(false);
    check_empty_allowed_replan(true);
}
}


kernel_fn! {
/// Two append-only request versions, each at its own admitted planner Await.
fn planner_entry_fixture(fail_fast: bool) -> (w: World) [
    ensures world_checked(w), world_headroom(w), storage_ordered(w),
        empty_replan::admitted(w, NodeId(0), 3), empty_replan::admitted(w, NodeId(1), 3),
        w.nodes@.len() == 2, w.next_question == 7, w.fail_fast == fail_fast,
        forall|j: int| 0 <= j < 2 ==> w.nodes@[j].generation == 3
            && w.nodes@[j].phase == (Phase::Planning { must_split: false }),
] proof [reveal(initial_node); reveal(world_checked); reveal(node_checked);
    reveal(world_headroom); reveal(storage_ordered); reveal(footprint::ordered_table);
    reveal(empty_replan::admitted); reveal(open_prefix);] {
    let mut n = Node::new(RepoId(0));
    n.phase = Phase::Planning { must_split: false };
    n.pos = 1;
    n.generation = 3;
    n.run = Some(3);
    n.pool = true;
    n.credit.ready = true;
    n.credit.start = 4;
    let mut nodes = Vec::new();
    nodes.push(n);
    nodes.push(n);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    let w = World { nodes, gates, pool_capacity: 2, next_ticket: 0, next_question: 7,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast };
    kernel_proof! { footprint::inline_is_valid(w, 0); }
    w
}
}

kernel_fn! {
/// Execute first-use and refusal through the public entry, on real Await nodes.
fn check_planner_entry_decision(fail_fast: bool, consumed: bool) -> (result: ()) []
proof [reveal(empty_replan::empty_replan_transition); reveal(empty_replan::planner_question_counter);] {
    let w = planner_entry_fixture(fail_fast);
    kernel_snapshot! { before = w }
    let budget = ReplanBudget::import(NodeId(0), consumed);
    let (after, budget, _) = step_planner(w, budget, NodeId(0), 3, PlannerResult::EmptyAllowedReplan);
    let used = budget.used();
    kernel_proof! {
        assert(used);
        assert(after.nodes@[0].question.is_none());
        assert(after.nodes@[0].credit == before@.nodes@[0].credit);
        assert(after.nodes@[0].plan_retries == before@.nodes@[0].plan_retries);
        assert(after.nodes@[0].replanned == before@.nodes@[0].replanned);
        assert(after.next_question == 7);
        assert(!consumed ==> after.nodes@[0].generation == 4
            && after.nodes@[0].phase == Phase::Planning { must_split: false });
        assert(consumed ==> after.nodes@[0].review_stop == Some(ReviewStop::PlanCantMeet)
            && (after.nodes@[0].phase is Stopping || after.nodes@[0].phase is Closed));
    }
}
}

kernel_fn! {
/// Stale generation and another request's key cannot consume permission.
fn check_planner_entry_isolation(fail_fast: bool, wrong_key: bool) -> (result: ()) []
proof [reveal(empty_replan::admitted);] {
    let w = planner_entry_fixture(fail_fast);
    kernel_snapshot! { before = w }
    let budget = ReplanBudget::new(if wrong_key { NodeId(1) } else { NodeId(0) });
    let generation = if wrong_key { 3 } else { 2 };
    let (after, budget, out) = step_planner(w, budget, NodeId(0), generation, PlannerResult::EmptyAllowedReplan);
    let used = budget.used();
    kernel_proof! { assert(!used); assert(after == before@); assert(out@.len() == 0); }
}
}

kernel_fn! {
/// New NodeId gets permission even while the older request remains consumed.
fn check_planner_entry_new_version(fail_fast: bool) -> (result: ()) []
proof [reveal(empty_replan::empty_replan_transition);] {
    let w = planner_entry_fixture(fail_fast);
    let old_budget = ReplanBudget::import(NodeId(0), true);
    let fresh = ReplanBudget::new(NodeId(1));
    let (after, fresh, _) = step_planner(w, fresh, NodeId(1), 3, PlannerResult::EmptyAllowedReplan);
    let old_used = old_budget.used();
    let fresh_used = fresh.used();
    kernel_proof! {
        assert(old_used && fresh_used);
        assert(old_budget.request_version() == NodeId(0));
        assert(fresh.request_version() == NodeId(1));
        assert(after.nodes@[1].generation == 4);
        assert(after.nodes@[1].phase == Phase::Planning { must_split: false });
    }
}
}

kernel_fn! {
fn apply_planner_entry_cases() -> (result: ()) [] {
    check_planner_entry_decision(false, false);
    check_planner_entry_decision(true, false);
    check_planner_entry_decision(false, true);
    check_planner_entry_decision(true, true);
    check_planner_entry_isolation(false, false);
    check_planner_entry_isolation(true, false);
    check_planner_entry_isolation(false, true);
    check_planner_entry_isolation(true, true);
    check_planner_entry_new_version(false);
    check_planner_entry_new_version(true);
}
}
