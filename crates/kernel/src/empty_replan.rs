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
                empty_replan_transition(world, result.0, request, budget.consumed()),
] proof [reveal(ReplanBudget::request_version); reveal(ReplanBudget::consumed); reveal(empty_replan_transition);
    reveal(admitted); reveal(world_headroom); world_checked_definition(world);] {
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
        reveal(settle_ready); reveal(scheduling_safe);
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
