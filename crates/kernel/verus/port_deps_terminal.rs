//! #693 / #806 W7: terminality and waiting over arbitrary prerequisite tables.
use super::port_deps_specs::*;
use super::*;

verus! {
/// State-table terminal prerequisite facts / #806: every subsequent observation
/// of a terminal key is silent and leaves the whole real World unchanged.
pub(super) fn terminal_stutters(mut world: World, i: usize,
    target: DependencyTarget, status: DependencyStatus) -> (result: (World, Outbox))
    requires step_scope(world), i < world.nodes@.len(), i <= u64::MAX as int,
        terminal_key(world, i as int, target),
    ensures unchanged(world, result.0), result.1.len == 0,
{
    let ghost before = world;
    let mut out = Outbox::new();
    proof { reveal(step_scope); reveal(empty_outbox); }
    apply(&mut world, Event::Fact { node: NodeId(i as u64), fact: Fact::Dependency { target, status } }, &mut out);
    proof { reveal(handler_post); }
    (world, out)
}

/// State-table prerequisite wait / #806: a blocked Unplanned root cannot own
/// delivery resources or spendable credit, and the real advance is a full stutter.
pub(super) fn waiting_holds_nothing(mut world: World, i: usize, blocker: usize)
    -> (result: (World, Outbox, i128))
    requires world_checked(world), world_headroom(world), storage_ordered(world),
        i < world.nodes@.len(), i <= u64::MAX as int,
        world.nodes@[i as int].phase == Phase::Unplanned,
        blocker < world.dependencies@.len(),
        world.dependencies@[blocker as int].node == NodeId(i as u64),
        world.dependencies@[blocker as int].rejected
            || world.dependencies@[blocker as int].status != DependencyStatus::Delivered,
    ensures unchanged(world, result.0), result.1.len == 0, result.2 == 0,
        !result.0.nodes@[i as int].holds && !result.0.nodes@[i as int].pool
            && result.0.nodes@[i as int].ticket.is_none() && result.0.nodes@[i as int].run.is_none(),
{
    let ghost before = world;
    proof {
        world_checked_definition(world);
        dependencies::delivered_definition(world.dependencies@, NodeId(i as u64));
        dependencies::prerequisite_definition(world, NodeId(i as u64));
        assert(!dependencies::prerequisites_delivered(world, NodeId(i as u64)));
        assert(dependencies::dependency_checked(world, world.dependencies@[blocker as int]));
        reveal(dependencies::zero_credit);
        reveal(storage_ordered); checked_reserves(world); reveal(settle_ready);
        potential_member(world.nodes@, i as int, world.nodes@.len(), |n: Node| ticket_capacity(n));
    }
    let mut out = Outbox::new();
    proof { reveal(empty_outbox); }
    let moved = advance(&mut world, i, &mut out);
    assert(!moved);
    proof { reveal(advance_post); reveal(unchanged); assert(world.nodes@ =~= before.nodes@); }
    let balance = world.nodes[i].credit.balance();
    (world, out, balance)
}
}
