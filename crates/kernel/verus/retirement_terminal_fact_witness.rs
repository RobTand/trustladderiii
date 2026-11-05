//! #806: changed-world covers for terminal merges and safety facts.
use super::retirement::*;
use super::*;

kernel_fn! {
/// Merge rows / #806: six concrete worlds satisfy the original constructor
/// bounds and consistency, including pending teardown, late correction and both conflict phases.
fn terminal_fact_world(case: u8) -> (result: World) [
    requires case < 6,
    ensures domain(result), accepted(result), bounded(result), storage_ordered(result), settled_world(result),
        result.nodes@[0].phase == match case {
            0 => Phase::Stopping(Outcome::Stopped),
            1 => Phase::Closed(Outcome::Stopped),
            4 => Phase::Publishing,
            _ => Phase::Queued,
        },
        result.nodes@[0].had_pr && result.nodes@[0].merged.is_none(),
        result.nodes@[0].priority == Priority::P3,
        result.nodes@[0].pr_open == (case >= 2),
        !result.nodes@[0].credit.merge_credited,
] proof [reveal(initial_node); reveal(accepted); reveal(node_rules); reveal(bounded);
    reveal(storage_ordered); reveal(footprint::ordered_table);] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    second.phase = Phase::Closed(Outcome::Stopped);
    first.phase = match case {
        0 => Phase::Stopping(Outcome::Stopped),
        1 => Phase::Closed(Outcome::Stopped),
        4 => Phase::Publishing,
        _ => Phase::Queued,
    };
    first.credit.ready = true;
    first.credit.start = 4;
    first.had_pr = true;
    first.head = Some(HeadId(1));
    first.checked = first.head;
    first.approved = first.head;
    first.attempt = 1;
    first.footprint = Footprint::inline(1);
    first.priority = Priority::P3;
    first.holds = case >= 2;
    first.pr_open = case >= 2;
    if case == 0 {
        first.run = Some(0);
        first.pool = true;
        first.next_seq = 1;
        first.effect = Some(InFlight { seq: 0, kind: EffectKind::StopRun, cancelling: false });
    } else if case == 4 {
        first.pos = 3;
        first.next_seq = 1;
        first.effect = Some(InFlight { seq: 0, kind: EffectKind::Enqueue, cancelling: false });
    }
    let mut nodes = Vec::new();
    nodes.push(first);
    nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    gates.push(Gate { w: 1 });
    let world = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 0,
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
/// #806 covers: terminal merges, changed priority, a closed PR and both
/// conflict phases. Six bounded settled witnesses cover five Kani harnesses.
/// One real apply, no unwind. Universal proofs cover all outcomes and priorities.
fn terminal_fact_changed_world(case: u8) -> (result: ()) [
    requires case < 6,
] proof [reveal(fact_node); reveal(stopped);] {
    let before = terminal_fact_world(case);
    kernel_snapshot! { original = before }
    let fact = match case {
        0 | 1 => Fact::PrMerged { head: HeadId(2), checks_passed: false },
        2 => Fact::Priority(Priority::P0),
        3 => Fact::PrClosedByPerson,
        _ => Fact::Conflict,
    };
    let (after, out) = verify_selected_apply(before, fact);
    kernel_proof! {
        assert(after.nodes@[0] != original@.nodes@[0]);
        if case <= 1 {
            assert(after.nodes@[0].merged == Some(HeadId(2)));
            if case == 0 {
                assert(after.nodes@[0].phase == Phase::Stopping(Outcome::Merged));
                assert(unapproved_note(out, 0));
            } else {
                assert(after.nodes@[0].phase == Phase::Closed(Outcome::Merged));
                assert(after.nodes@[0].credit.merge_credited);
                assert(after.nodes@[0].credit.earned == original@.nodes@[0].credit.earned + 1);
            }
        } else if case == 2 {
            assert(after.nodes@[0] == Node { priority: Priority::P0, ..original@.nodes@[0] });
            assert(out.len == 0);
        } else if case == 3 {
            assert(!after.nodes@[0].pr_open);
            assert(after.nodes@[0].phase == Phase::Stopping(Outcome::Stopped));
        } else {
            assert(after.nodes@[0].phase == Phase::Rebasing);
            assert(after.nodes@[0].generation == original@.nodes@[0].generation + 1);
            assert(after.nodes@[0].credit == original@.nodes@[0].credit);
            assert(after.nodes@[0].holds == original@.nodes@[0].holds);
        }
    }
}
}
