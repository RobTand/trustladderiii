//! Q27, Rob 2026-11-06: one empty-allowed re-plan per request version.
use super::*;

/// Kernel state persisted in the existing requests/<NodeId> row, outside World.
/// Only creation of a new request version may call `new`; generations reuse it.
#[cfg_attr(all(feature = "serde", not(kani)), derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct ReplanBudget {
    request: NodeId,
    used: bool,
}

#[cfg(verus_keep_ghost)]
verus! {
impl ReplanBudget {
    pub closed spec fn request_version(&self) -> NodeId { self.request }
    pub closed spec fn consumed(&self) -> bool { self.used }
}
}

kernel_fn! {
impl ReplanBudget {
    pub fn import(request: NodeId, consumed: bool) -> (result: ReplanBudget) [
        ensures result.request_version() == request, result.consumed() == consumed,
    ] proof [reveal(ReplanBudget::request_version); reveal(ReplanBudget::consumed);] {
        ReplanBudget { request, used: consumed }
    }

}
}

kernel_fn! {
impl ReplanBudget {
    pub fn bound_to(&self, request: NodeId) -> (result: bool) [
        ensures result == (self.request_version() == request),
    ] proof [reveal(ReplanBudget::request_version);] {
        self.request == request
    }

}
}

kernel_fn! {
impl ReplanBudget {
    pub fn new(request: NodeId) -> (result: ReplanBudget) [
        ensures result.request_version() == request, !result.consumed(),
    ] proof [reveal(ReplanBudget::request_version); reveal(ReplanBudget::consumed);] {
        ReplanBudget { request, used: false }
    }
}
}

kernel_fn! {
impl ReplanBudget {
    pub fn used(&self) -> (result: bool) [ensures result == self.consumed(),] proof [reveal(ReplanBudget::consumed);] {
        self.used
    }
}
}

/// Separate from Reply and Event, so dependency-reachable types carry no budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(all(feature = "serde", not(kani)), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub enum PlannerResult {
    Ordinary(Reply),
    EmptyAllowedReplan,
}

#[cfg(verus_keep_ghost)]
verus! {
pub closed spec fn planner_question_counter(w: World) -> u64 { w.next_question }

/// This is the executable entry's pre-consumption admission, not caller policy.
pub closed spec fn admitted(w: World, request: NodeId, generation: u64) -> bool {
    request.0 < w.nodes@.len() && {
        let n = w.nodes@[request.0 as int];
        n.phase is Planning && current_item(n) == Some(Item::Await)
            && (n.run == Some(generation) || n.job == Some(generation))
    }
}

pub closed spec fn empty_replan_transition(world: World, next: World,
    request: NodeId, consumed: bool) -> bool {
    let before = world.nodes@[request.0 as int];
    let after = next.nodes@[request.0 as int];
    after.question.is_none() && after.credit == before.credit
        && after.plan_retries == before.plan_retries && after.replanned == before.replanned
        && (!consumed ==> after.phase == before.phase
            && after.generation as int == before.generation as int + 1)
        && (consumed ==> after.review_stop == Some(ReviewStop::PlanCantMeet)
            && (after.phase is Stopping || after.phase is Closed))
}

/// Settling advances agendas without changing the planner's retry decision.
pub(super) open spec fn settle_frame(before: Node, after: Node) -> bool {
    after.plan_retries == before.plan_retries && after.replanned == before.replanned
        && after.review_stop == before.review_stop
        && (before.question.is_none() ==> after.question.is_none())
        && (before.phase is Planning ==> after.phase == before.phase
            && after.generation == before.generation && after.credit == before.credit)
        && (before.phase == Phase::Stopping(Outcome::Stopped) ==>
            (after.phase == before.phase || after.phase == Phase::Closed(Outcome::Stopped))
                && after.credit == before.credit)
}
}

kernel_fn! {
/// Request-version planner entry. Kernel code alone consumes the budget.
/// Request binding precedes consumption. Only re-plans use special admission.
/// Ordinary replies retain step's stopping, stale-reply and settling behavior.
pub fn step_planner(mut world: World, mut budget: ReplanBudget, request: NodeId,
    generation: u64, planner_result: PlannerResult)
    -> (result: (World, ReplanBudget, Vec<Out>)) [
    requires world_checked(world), world_headroom(world), storage_ordered(world),
    ensures result.1.request_version() == budget.request_version(),
        result.1.consumed() == (budget.consumed() || (budget.request_version() == request
            && admitted(world, request, generation) && planner_result is EmptyAllowedReplan)),
        (budget.request_version() != request || (planner_result is EmptyAllowedReplan
            && !admitted(world, request, generation))) ==>
            result.0 == world && result.2@.len() == 0,
        (budget.request_version() == request && admitted(world, request, generation)
            && planner_result is EmptyAllowedReplan) ==>
                empty_replan_transition(world, result.0, request, budget.consumed())
                    && planner_question_counter(result.0) == planner_question_counter(world),
] proof [reveal(ReplanBudget::request_version); reveal(ReplanBudget::consumed); reveal(empty_replan_transition);
    reveal(admitted); reveal(planner_question_counter); reveal(world_headroom); world_checked_definition(world);] {
    if budget.request != request {
        return (world, budget, Vec::new());
    }
    match planner_result {
        PlannerResult::Ordinary(reply) => {
            let mut mapped = reply;
            if let Some(i) = request.index() {
                if i < world.nodes.len() {
                    let n = &world.nodes[i];
                    if matches!(n.phase, Phase::Planning { .. }) && n.current() == Some(Item::Await)
                        && (n.run == Some(generation) || n.job == Some(generation)) {
                        let accepted = accepts(&world, n, &reply);
                        let semantic_failure = match reply {
                            Reply::PlanCantMeet | Reply::PlanSplit => true,
                            Reply::PlanLeaf { footprint, fits } => !fits || footprint.is_empty()
                                || matches!(n.phase, Phase::Planning { must_split: true }),
                            _ => false,
                        };
                        if accepted && budget.used && semantic_failure {
                            mapped = Reply::PlanCheckRejected;
                        }
                    }
                }
            }
            let (world, out) = step(world, Event::Reply { node: request, generation, reply: mapped });
            return (world, budget, out);
        }
        PlannerResult::EmptyAllowedReplan => {}
    }
    let i = match request.index() {
        Some(i) => i,
        None => return (world, budget, Vec::new()),
    };
    if i >= world.nodes.len() {
        return (world, budget, Vec::new());
    }
    let n = &world.nodes[i];
    if !matches!(n.phase, Phase::Planning { .. }) || n.current() != Some(Item::Await)
        || (n.run != Some(generation) && n.job != Some(generation)) {
        return (world, budget, Vec::new());
    }
    kernel_snapshot! { before_replan = world }
    let mut outbox = Outbox::new();
    kernel_proof! { reveal(empty_outbox); assert(node_checked(world, world.nodes@[i as int])); }
    end_operation(&mut world.nodes[i]);
    reply_empty_replan(&mut world, i, &mut budget, &mut outbox);
    kernel_proof! {
        reveal(reply_bounds); reveal(storage_ordered);
        reveal(settle_ready); reveal(scheduling_safe); reveal(node_checked);
        reveal(footprint::ordered_table);
        assert forall|j: int| 0 <= j < world.nodes@.len() implies
            settle_shape(world.nodes@[j])
                && world.nodes@[j].generation as int + generation_capacity(world.nodes@[j]) <= u64::MAX as int
                && world.nodes@[j].next_seq as int + effect_capacity(world.nodes@[j]) <= u64::MAX as int
                && world.nodes@[j].credit.earned as int + merge_capacity(world.nodes@[j]) <= u64::MAX as int
        by {
            assert(node_checked(before_replan@, before_replan@.nodes@[j]));
            footprint::valid_under_table_growth(before_replan@, world, world.nodes@[j].footprint);
            local_capacities(world.nodes@[j]);
            if j != i { assert(world.nodes@[j] == before_replan@.nodes@[j]); }
        }
        assert forall|j: int| 0 <= j < world.nodes@.len() implies
            footprint::valid_footprint(world, world.nodes@[j].footprint)
                && ((world.nodes@[j].holds || world.nodes@[j].pool) ==> !(world.nodes@[j].phase is Closed))
        by {
            footprint::valid_under_table_growth(before_replan@, world, world.nodes@[j].footprint);
            if j != i { assert(world.nodes@[j] == before_replan@.nodes@[j]); }
        }
        open_prefix_compare(before_replan@.nodes@, world.nodes@, before_replan@.nodes@.len());
        settle_reserve_totals(world.nodes@);
        open_prefix_monotone(world.nodes@, 0, world.nodes@.len());
        reveal(open_prefix);
        assert(scheduling_safe(world));
        assert(world.next_ticket as int + potential_prefix(world.nodes@, world.nodes@.len(), |n: Node| ticket_capacity(n)) <= u64::MAX as int);
        assert(potential_prefix(world.nodes@, world.nodes@.len(), |n: Node| internal_height(n)) <= u32::MAX as int);
        assert(settle_ready(world));
    }
    let mut out = Vec::new();
    outbox.drain_into(&mut out);
    kernel_snapshot! { after_replan = world }
    settle(&mut world, &mut out, None);
    kernel_proof! {
        reveal(settle_chain);
        assert(settle_frame(after_replan@.nodes@[i as int], world.nodes@[i as int]));
    }
    (world, budget, out)
}
}

kernel_fn! {
/// Consume once or stop for review without spending credit or asking a question.
/// The usual Planning agenda starts the tool-enabled run.
pub(super) fn reply_empty_replan(w: &mut World, i: usize, budget: &mut ReplanBudget, _out: &mut Outbox)
    -> (result: ()) [
    requires i < old(w).nodes@.len(), old(_out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        old(w).nodes@[i as int].phase is Planning,
        old(budget).request_version().0 == i as int,
    ensures reply_bounds(*old(w), *final(w), i as int, old(_out).len, final(_out).len),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(budget).request_version() == old(budget).request_version(), final(budget).consumed(),
        (!old(budget).consumed() ==>
            final(w).nodes@[i as int] == retirement::phased(
                Node { question: None, ..old(w).nodes@[i as int] },
                old(w).nodes@[i as int].phase)),
        (old(budget).consumed() ==>
            final(w).nodes@[i as int].phase == Phase::Stopping(Outcome::Stopped)
                && final(w).nodes@[i as int].review_stop == Some(ReviewStop::PlanCantMeet)),
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).nodes@[i as int].question.is_none(),
        final(w).nodes@[i as int].plan_retries == old(w).nodes@[i as int].plan_retries,
        final(w).nodes@[i as int].replanned == old(w).nodes@[i as int].replanned,
        final(w).next_question == old(w).next_question,
        *final(_out) == *old(_out),
] proof [reveal(ReplanBudget::request_version); reveal(ReplanBudget::consumed); reveal(reply_bounds); reveal(retirement::phased); reveal(retirement::stopped);] {
    let m = &mut w.nodes[i];
    m.question = None;
    if !budget.used {
        budget.used = true;
        let phase = m.phase;
        set_phase(m, phase);
    } else {
        stop_for_review(m, ReviewStop::PlanCantMeet);
    }
}
}


#[cfg(verus_keep_ghost)]
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

#[cfg(verus_keep_ghost)]
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

#[cfg(verus_keep_ghost)]
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

#[cfg(verus_keep_ghost)]
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

#[cfg(verus_keep_ghost)]
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
