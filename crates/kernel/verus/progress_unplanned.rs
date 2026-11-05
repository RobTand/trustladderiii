//! #806 B1: Unplanned outside selection, apply-only decrease, and node frame.
use super::*;

verus! {
/// #806: same two-node construction, without settledness or finite edge bounds.
pub(super) open spec fn scope(w: World) -> bool {
    w.nodes@.len() == 2 && w.nodes@[0].phase == Phase::Unplanned
        && world_checked(w) && world_headroom(w) && storage_ordered(w) && fresh_commands(w)
}

/// State-table Unplanned: the three outside obstructions, in selector order.
pub(super) open spec fn obstructed(n: Node) -> bool {
    n.paused || n.effect.is_some() || n.waiting_incident
}

/// Q30 / #806: exact fresh Resume, current confirmation, or Rob incident clear.
pub(super) open spec fn outside_event(n: Node) -> Event {
    if n.paused { Event::Command { node: NodeId(0), id: (n.last_cmd + 1) as u64,
        command: Command::Resume } }
    else if let Some(e) = n.effect { Event::Ack { node: NodeId(0), seq: e.seq, ack: Ack::Confirmed } }
    else { Event::Fact { node: NodeId(0), fact: Fact::IncidentCleared { by_rob: true } } }
}

/// #806: the original small-counter assumptions imply all #805 reserves.
proof fn original_bounds_supply_reserves(w: World)
    requires w.nodes@.len() == 2,
        w.next_ticket < 1099511627776u64, w.next_question < 1099511627776u64,
        forall|i: int| 0 <= i < 2 ==> w.nodes@[i].generation < 1099511627776u64
            && w.nodes@[i].next_seq < 1099511627776u64 && w.nodes@[i].last_cmd < 1099511627776u64
            && w.nodes@[i].credit.start < 1073741824u64 && w.nodes@[i].credit.earned < 1073741824u64
            && w.nodes@[i].credit.added < 1073741824u64 && w.nodes@[i].credit.spent < 1073741824u64,
    ensures world_headroom(w), fresh_commands(w),
{
    reveal(world_headroom); reveal(fresh_commands);
    open_prefix_monotone(w.nodes@, 0, w.nodes@.len()); reveal(open_prefix);
}
}

verus! {
/// #806: real witness and real apply, even while the other root can still settle.
pub(super) fn verify_outside(mut world: World) -> (result: World)
    requires scope(world), obstructed(world.nodes@[0]),
    ensures result.nodes@[1] == world.nodes@[1],
        progress_measure(result) < progress_measure(world),
{
    let ghost before = world;
    proof {
        reveal(storage_ordered); checked_reserves(world); reveal(step_scope);
    }
    let event = world.witness();
    proof {
        assert(event == Some(outside_event(before.nodes@[0])));
        reveal(progress_input); reveal(witness_event); reveal(chosen_for_node);
        assert(chosen_for_node(before, 0, event.unwrap()));
        assert(progress_input(before, event.unwrap()));
    }
    let mut out = Outbox::new();
    proof { reveal(empty_outbox); }
    apply(&mut world, event.unwrap(), &mut out);
    world
}
}
