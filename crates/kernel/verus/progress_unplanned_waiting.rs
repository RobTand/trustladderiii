//! #693 / #806 B1: arbitrary finite blocked prerequisite tables cannot admit.
use super::*;
use super::progress_unplanned::*;

verus! {
/// #806: equality of every observable world field, including declaration storage.
pub(super) open spec fn unchanged(a: World, b: World) -> bool {
    a.nodes@ == b.nodes@ && a.gates@ == b.gates@ && a.dependencies@ == b.dependencies@
        && a.file_sets@ == b.file_sets@ && a.pool_capacity == b.pool_capacity
        && a.next_ticket == b.next_ticket && a.next_question == b.next_question && a.fail_fast == b.fail_fast
}

/// State-table Unplanned wait: no move, output, resource ownership, or credit.
pub(super) fn verify_waiting(mut world: World, blocker: usize) -> (result: World)
    requires scope(world), blocker < world.dependencies@.len(),
        world.dependencies@[blocker as int].node == NodeId(0),
        world.dependencies@[blocker as int].rejected
            || world.dependencies@[blocker as int].status != DependencyStatus::Delivered,
    ensures unchanged(world, result), !result.nodes@[0].holds && !result.nodes@[0].pool
        && result.nodes@[0].ticket.is_none() && result.nodes@[0].run.is_none(),
{
    let ghost before = world;
    proof {
        reveal(dependencies::delivered);
        assert(!dependencies::delivered(world.dependencies@, NodeId(0)));
        reveal(storage_ordered); checked_reserves(world); reveal(settle_ready);
        potential_member(world.nodes@, 0, world.nodes@.len(), |n: Node| ticket_capacity(n));
    }
    let delivered = world.dependencies_delivered(NodeId(0));
    assert(!delivered);
    let mut out = Outbox::new();
    proof { reveal(empty_outbox); }
    let moved = advance(&mut world, 0, &mut out);
    assert(!moved);
    proof {
        reveal(advance_post); reveal(unchanged);
        assert(unchanged(before, world));
        assert(out.len == 0);
    }
    let balance = world.nodes[0].credit.balance();
    assert(balance == 0);
    world
}
}
