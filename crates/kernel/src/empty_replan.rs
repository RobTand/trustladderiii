//! Q27, Rob 2026-11-06: one empty-allowed re-plan per request version.
use super::*;

/// Kernel state persisted in the existing requests/<NodeId> row, outside World.
/// Only creation of a new request version may call `new`; generations reuse it.
#[cfg_attr(all(feature = "serde", not(kani)), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct ReplanBudget {
    request: NodeId,
    used: bool,
}

kernel_fn! {
impl ReplanBudget {
    pub fn new(request: NodeId) -> (result: ReplanBudget) [
        ensures result.request == request, !result.used,
    ] {
        ReplanBudget { request, used: false }
    }
}
}

kernel_fn! {
impl ReplanBudget {
    pub fn used(&self) -> (result: bool) [ensures result == self.used,] {
        self.used
    }
}
}

/// Separate from Reply and Event, so dependency-reachable types carry no budget.
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
}

kernel_fn! {
/// Draft public production entry. Its composition contracts are UNVERIFIED.
/// Request binding and stale admission precede consumption and operation clearing.
/// Ordinary replies share on_reply's schema check, end_operation and dispatch.
pub fn step_planner(mut world: World, mut budget: ReplanBudget, request: NodeId,
    generation: u64, planner_result: PlannerResult)
    -> (result: (World, ReplanBudget, Vec<Out>)) [
    requires world_checked(world), world_headroom(world), storage_ordered(world),
    ensures result.1.request == budget.request,
        result.1.used == (budget.used || (budget.request == request
            && admitted(world, request, generation) && planner_result is EmptyAllowedReplan)),
        (budget.request != request || !admitted(world, request, generation)) ==>
            result.0 == world && result.2@.len() == 0,
        (budget.request == request && admitted(world, request, generation)
            && planner_result is EmptyAllowedReplan) ==> {
                let before = world.nodes@[request.0 as int];
                let after = result.0.nodes@[request.0 as int];
                after.question.is_none() && after.credit == before.credit
                    && after.plan_retries == before.plan_retries && after.replanned == before.replanned
                    && (!budget.used ==> after.phase == before.phase
                        && after.generation as int == before.generation as int + 1)
                    && (budget.used ==> after.review_stop == Some(ReviewStop::PlanCantMeet)
                        && (after.phase is Stopping || after.phase is Closed))
            },
] {
    if budget.request != request {
        return (world, budget, Vec::new());
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
    let mut outbox = Outbox::new();
    match planner_result {
        PlannerResult::EmptyAllowedReplan => {
            end_operation(&mut world.nodes[i]);
            reply_empty_replan(&mut world, i, &mut budget, &mut outbox);
        }
        PlannerResult::Ordinary(reply) => {
            // Check schema before mapping a consumed semantic failure to review.
            // Invalid footprint replies retain the existing failed-run handling.
            let accepted = accepts(&world, &world.nodes[i], &reply);
            let semantic_failure = match reply {
                Reply::PlanCantMeet | Reply::PlanSplit => true,
                Reply::PlanLeaf { footprint, fits } => !fits || footprint.is_empty()
                    || matches!(world.nodes[i].phase, Phase::Planning { must_split: true }),
                _ => false,
            };
            let reply = if accepted && budget.used && semantic_failure {
                Reply::PlanCheckRejected
            } else {
                reply
            };
            on_reply(&mut world, i, generation, reply, &mut outbox);
        }
    }
    let mut out = Vec::new();
    outbox.drain_into(&mut out);
    settle(&mut world, &mut out, None);
    (world, budget, out)
}
}

kernel_fn! {
/// Draft contract for the actual consume/refuse body, not selected by this campaign.
/// The usual Planning agenda starts the tool-enabled run.
pub(super) fn reply_empty_replan(w: &mut World, i: usize, budget: &mut ReplanBudget, _out: &mut Outbox)
    -> (result: ()) [
    requires i < old(w).nodes@.len(), old(_out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        old(w).nodes@[i as int].phase is Planning,
        old(budget).request.0 == i as int,
    ensures reply_bounds(*old(w), *final(w), i as int, old(_out).len, final(_out).len),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(budget).request == old(budget).request, final(budget).used,
        (!old(budget).used ==>
            final(w).nodes@[i as int] == retirement::phased(
                Node { question: None, ..old(w).nodes@[i as int] },
                old(w).nodes@[i as int].phase)),
        (old(budget).used ==>
            final(w).nodes@[i as int].phase == Phase::Stopping(Outcome::Stopped)
                && final(w).nodes@[i as int].review_stop == Some(ReviewStop::PlanCantMeet)),
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).nodes@[i as int].question.is_none(),
        final(w).nodes@[i as int].plan_retries == old(w).nodes@[i as int].plan_retries,
        final(w).nodes@[i as int].replanned == old(w).nodes@[i as int].replanned,
        final(w).next_question == old(w).next_question,
        *final(_out) == *old(_out),
] proof [reveal(reply_bounds); reveal(retirement::phased); reveal(retirement::stopped);] {
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
