//! Q54 / #440 / #806: original-domain maker and planner diversion constructors.
use super::*;

kernel_fn! {
/// Q54: construct the original settled two-node world at a live maker or planner Await.
/// Bounds are the original four-file, two-repository, small-ledger construction.
pub(super) fn world(planning: bool, split: bool, funding: bool) -> (result: World) [
    ensures progress_phase_witness::original(result), retirement::accepted(result),
        retirement::bounded(result), settled_world(result), storage_ordered(result),
        world_headroom(result), world_checked(result),
        !result.fail_fast, result.next_question == 0,
        result.nodes@[0].phase == if planning { Phase::Planning { must_split: split } } else { Phase::Making },
        result.nodes@[0].generation == 0 && result.nodes@[0].run == Some(0),
        result.nodes@[0].job.is_none(),
        current_item(result.nodes@[0]) == Some(Item::Await),
        result.nodes@[0].effect.is_none() && result.nodes@[0].question.is_none(),
        result.nodes@[0].review_stop.is_none(),
        result.nodes@[0].credit.start == if funding { 0u64 } else { 4u64 },
        result.nodes@[0].credit.spent == 0 && result.nodes@[0].credit.earned == 0
            && result.nodes@[0].credit.added == 0,
        result.nodes@[0].plan_retries == 0 && result.nodes@[0].attempt == 1,
] proof [reveal(initial_node); reveal(retirement::accepted); reveal(retirement::node_rules);
    reveal(retirement::bounded); reveal(progress_phase_witness::original);] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    first.phase = if planning { Phase::Planning { must_split: split } } else { Phase::Making };
    first.credit.ready = true;
    first.credit.start = if funding { 0 } else { 4 };
    first.attempt = 1;
    first.footprint = Footprint::inline(1);
    first.holds = !planning;
    first.worktree = !planning;
    first.pos = if planning { 1 } else { 2 };
    first.run = Some(0);
    first.pool = true;
    second.phase = Phase::Closed(Outcome::Stopped);
    second.footprint = Footprint::inline(1);
    let mut nodes = Vec::new();
    nodes.push(first); nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 }); gates.push(Gate { w: 1 });
    let w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    kernel_proof! {
        footprint::inline_is_valid(w, 1);
        assert forall|i: int| 0 <= i < 2 implies retirement::node_rules(w, w.nodes@[i]) by {
            assert(i == 0 || i == 1);
        }
        assert(retirement::accepted(w));
        assert forall|i: int| 0 <= i < 2 implies {
            let n = #[trigger] w.nodes@[i];
            n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
                && n.credit.start < 1099511627776u64 && n.credit.earned < 1099511627776u64
                && n.credit.added < 1099511627776u64 && n.credit.spent < 1099511627776u64
        } by { assert(i == 0 || i == 1); }
        assert(retirement::bounded(w));
        assert(progress_phase::domain(w));
        assert(progress_phase_witness::original(w));
        progress_phase::reserves(w);
        reveal(retirement_domain); reveal(retirement_accepted);
    }
    let checked = w.check();
    assert!(checked.is_ok());
    kernel_proof! {
        reveal(storage_ordered);
        checked_reserves(w);
        progress_phase::zero_internal_height_definition(w);
        reveal_with_fuel(potential_prefix, 3);
        assert(potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)) == 0);
    }
    let settled = w.is_settled();
    assert!(settled);
    w
}
}

kernel_fn! {
/// Q54 / #440 paired-apply construction: copy the original world's actual node
/// and gate vectors, then enable fail_fast. Empty file/dependency tables are
/// exactly the original Kani construction, with no alternative state model.
pub(super) fn fast_copy(w: &World) -> (result: World) [
    requires w.file_sets@.len() == 0, w.dependencies@.len() == 0,
    ensures port_failfast_admission::same_world(*w, result), result.fail_fast,
] {
    let mut result = snapshot_world(w);
    result.fail_fast = true;
    kernel_proof! { assert(result.file_sets@ =~= w.file_sets@); }
    result
}
}
