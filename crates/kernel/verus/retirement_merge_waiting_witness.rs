//! #806: changed-world covers for the third five-harness merge slice.
use super::retirement::*;
use super::*;

kernel_fn! {
/// Merge rows / #806: five concrete worlds satisfy the original constructor
/// bounds and consistency, including a live effect and an outstanding question to Rob.
fn merge_waiting_world(case: u8) -> (result: World) [
    requires case < 5,
    ensures domain(result), accepted(result), bounded(result), storage_ordered(result), settled_world(result),
        result.nodes@[0].phase == match case {
            0 => Phase::Reviewing,
            1 => Phase::Publishing,
            2 => Phase::Queued,
            3 => Phase::Rebasing,
            _ => Phase::AwaitingRob,
        },
        result.nodes@[0].had_pr && result.nodes@[0].merged.is_none(),
] proof [reveal(initial_node); reveal(accepted); reveal(node_rules); reveal(bounded);
    reveal(storage_ordered); reveal(footprint::ordered_table);] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    second.phase = Phase::Closed(Outcome::Stopped);
    first.phase = match case {
        0 => Phase::Reviewing,
        1 => Phase::Publishing,
        2 => Phase::Queued,
        3 => Phase::Rebasing,
        _ => Phase::AwaitingRob,
    };
    first.credit.ready = true;
    first.credit.start = 4;
    first.had_pr = true;
    first.head = Some(HeadId(1));
    first.checked = first.head;
    first.approved = first.head;
    first.attempt = 1;
    first.footprint = Footprint::inline(1);
    first.holds = case < 4;
    if case == 0 || case == 3 {
        first.pos = 1;
        first.run = Some(0);
        first.pool = case == 0;
    } else if case == 1 {
        first.pr_open = true;
        first.pos = 3;
        first.next_seq = 1;
        first.effect = Some(InFlight { seq: 0, kind: EffectKind::Enqueue, cancelling: false });
    } else if case == 2 {
        first.pr_open = true;
    } else {
        first.question = Some(Question { id: 0, kind: QKind::EffectFailed,
            cont: Cont::Ingesting, cost: 0, clarified: false });
    }
    let mut nodes = Vec::new();
    nodes.push(first);
    nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    gates.push(Gate { w: 1 });
    let world = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: if case == 4 { 1 } else { 0 },
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    kernel_proof! {
        footprint::inline_is_valid(world, 0);
        footprint::inline_is_valid(world, 1);
        assert forall|i: int| 0 <= i < world.nodes@.len() implies {
            let n = #[trigger] world.nodes@[i];
            n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
                && n.credit.start < 1099511627776u64 && n.credit.earned < 1099511627776u64
                && n.credit.added < 1099511627776u64 && n.credit.spent < 1099511627776u64
        } by { assert(i == 0 || i == 1); }
        assert(node_rules(world, world.nodes@[0]));
        assert(node_rules(world, world.nodes@[1]));
        assert(accepted(world));
        reveal(settled_world);
        reveal(movable);
        assert forall|i: int| 0 <= i < world.nodes@.len() implies !movable(world, i) by {
            assert(i == 0 || i == 1);
        }
    }
    world
}
}

kernel_fn! {
/// #806 covers: the real apply records an unapproved merge and changes every
/// selected phase. One apply, two nodes, no unwind; universal safety is proved
/// by verify_selected_apply for all heads and check outcomes.
fn merge_waiting_changed_world(case: u8) -> (result: ()) [
    requires case < 5,
] proof [reveal(fact_node); reveal(stopped);] {
    let before = merge_waiting_world(case);
    kernel_snapshot! { original = before }
    let (after, out) = verify_selected_apply(before,
        Fact::PrMerged { head: HeadId(2), checks_passed: false });
    kernel_proof! {
        assert(after.nodes@[0] != original@.nodes@[0]);
        assert(after.nodes@[0].merged == Some(HeadId(2)));
        assert(after.nodes@[0].phase == Phase::Stopping(Outcome::Merged));
        assert(unapproved_note(out, 0));
    }
}
}
