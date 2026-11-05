//! #806 B1: real admission or movement of the other root's gate occupant.
use super::progress_unplanned::*;
use super::*;

verus! {
/// State-table Unplanned: real admission enters exactly unsplit Planning.
pub(super) fn verify_admission(mut world: World) -> (result: (World, bool, bool))
    requires scope(world), !obstructed(world.nodes@[0]),
        dependencies::delivered(world.dependencies@, NodeId(0)),
    ensures result.1 == (slot_waiters(world.nodes@, world.nodes@.len(), world.nodes@[0].repo)
            < world.gates@[world.nodes@[0].repo.0 as int].w),
        result.1 ==> result.0.nodes@[0].phase == (Phase::Planning { must_split: false })
        && result.0.nodes@[1] == world.nodes@[1]
        && progress_measure(result.0) < progress_measure(world),
        !result.1 ==> world.nodes@[1].phase is Ready && world.nodes@[1].ticket.is_some(),
        result.2 == !result.1,
{
    let ghost before = world;
    proof {
        reveal(storage_ordered); checked_reserves(world); reveal(settle_ready);
        world_checked_definition(world); reveal(node_checked);
        assert(node_checked(world, world.nodes@[0]));
        assert(node_checked(world, world.nodes@[1]));
        reveal(empty_outbox);
        potential_member(world.nodes@, 0, world.nodes@.len(), |n: Node| ticket_capacity(n));
    }
    let mut out = Outbox::new();
    let admitted = advance(&mut world, 0, &mut out);
    proof {
        if admitted {
            assert(world.nodes@[0].phase == (Phase::Planning { must_split: false }));
            reveal(distance);
            progress::lift_node_progress(before, world, 0);
        } else {
            reveal_with_fuel(potential_prefix, 3);
            assert(before.nodes@[1].phase is Ready && before.nodes@[1].ticket.is_some());
            reveal(advance_post); reveal(movable); reveal(oldest_waiter);
            reveal(scheduling_safe);
            assert(world.nodes@ == before.nodes@);
            assert(world.nodes@[0].ticket.is_none());
            assert(world.nodes@[1].effect.is_none());
            assert(movable(world, 1));
            potential_member(world.nodes@, 1, world.nodes@.len(), |n: Node| ticket_capacity(n));
        }
    }
    let mut occupant_moved = false;
    if !admitted {
        occupant_moved = advance(&mut world, 1, &mut out);
        assert(occupant_moved);
    }
    (world, admitted, occupant_moved)
}
}
