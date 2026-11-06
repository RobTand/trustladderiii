//! #806 W7: original-domain real scheduler composition.
use super::port_deps_pool::*;
use super::port_deps_pool_witness::*;
use super::port_deps_specs::*;
use super::*;

verus! {
/// Q39 / #806: project the exact local StartRun frame onto the original world.
pub(super) open spec fn pool_step(w: World, v: World, granted: bool) -> bool {
    v.nodes@ == w.nodes@.update(0, v.nodes@[0])
        && v.nodes@[0] == (if granted { Node {
            ticket: None, pool: true, run: Some(w.nodes@[0].generation),
            pos: (w.nodes@[0].pos + 1) as u8, next_seq: (w.nodes@[0].next_seq + 1) as u64,
            effect: Some(InFlight { seq: w.nodes@[0].next_seq,
                kind: EffectKind::StartRun(Role::Ingest), cancelling: false }), ..w.nodes@[0]
        } } else { w.nodes@[0] })
        && v.next_ticket == w.next_ticket && v.next_question == w.next_question
        && v.gates@ == w.gates@ && v.pool_capacity == w.pool_capacity
        && v.fail_fast == w.fail_fast && v.file_sets@ == w.file_sets@
        && v.dependencies@ == w.dependencies@
}

/// Q39 / #806: the exact original pool step preserves every complete checker
/// condition. Isolate this from the existential scheduler-context query.
pub(super) proof fn grant_is_checked(w: World, v: World, granted: bool)
    requires pool_fixture(w), pool_step(w, v, granted),
    ensures checker_accepted(v), world_headroom(v), storage_ordered(v),
{
    reveal(pool_fixture); reveal(pool_node); reveal(initial_node); reveal(pool_step);
    reveal(checker_accepted); reveal(retirement::node_rules); reveal(node_checked);
    reveal(world_headroom); reveal(storage_ordered); reveal(footprint::ordered_table);
    footprint::inline_is_valid(v, 0); reveal(pool_users);
    reveal_with_fuel(open_prefix, 4); reveal_with_fuel(potential_prefix, 4);
    assert forall|i: int| 0 <= i < 3 implies
        retirement::node_rules(v, v.nodes@[i]) && !v.nodes@[i].holds
        by { assert(i == 0 || i == 1 || i == 2); }
    assert forall|i: int, j: int| 0 <= i < j < 3 && v.nodes@[i].ticket.is_some()
        implies v.nodes@[i].ticket != v.nodes@[j].ticket by {
        assert(i == 0 || i == 1); assert(j == 1 || j == 2);
    }
}

/// Q39 / #806: the original arbitrary-priority three-root grant, using the
/// independent level/ticket minimum and the real complete checker afterward.
pub(super) fn pool_grant(mut world: World) -> (result: (World, Outbox))
    requires pool_fixture(world), world_checked(world), world_headroom(world),
        storage_ordered(world), world.nodes@[0].ticket.is_some(),
    ensures result.0.nodes@[0].pool == best(world, 0),
        result.0.pool_capacity == world.pool_capacity,
        result.0.nodes@[0].pool ==> result.0.nodes@[0].ticket.is_none(),
        !result.0.nodes@[0].pool ==> result.0.nodes@[0] == world.nodes@[0],
        result.0.nodes@[1] == world.nodes@[1] && result.0.nodes@[2] == world.nodes@[2],
        world_checked(result.0),
{
    let ghost before = world;
    let mut out = Outbox::new();
    let ghost before_out = out;
    proof {
        reveal(pool_fixture); reveal(pool_node); reveal(initial_node);
        reveal(storage_ordered); checked_reserves(world); reveal(settle_ready);
        potential_member(world.nodes@, 0, world.nodes@.len(), |n: Node| ticket_capacity(n));
        reveal(empty_outbox); line_definition(world, 0);
        reveal(pool_users); reveal_with_fuel(potential_prefix, 4);
    }
    let moved = advance(&mut world, 0, &mut out);
    proof {
        let c = choose|c: AdvanceContext| c.node == 0
            && moved == advance_enabled(before.nodes@[0], c)
            && port_advance_contracts::context_safe(before, 0, c)
            && port_advance_contracts::pool_context(before, 0, c)
            && port_advance_contracts::exact(before.nodes@[0], world.nodes@[0],
                before.next_ticket, world.next_ticket, c, before_out, out);
        port_advance_contracts::pool_frame(before.nodes@[0], world.nodes@[0],
            before.next_ticket, world.next_ticket, c, before_out, out, Role::Ingest, moved);
        assert(world.nodes@[0].pool == best(before, 0));
        assert(pool_step(before, world, moved));
        grant_is_checked(before, world, moved); reveal(port_deps_checker_accepted);
    }
    let accepted = world.check();
    assert(accepted.is_ok());
    (world, out)
}

/// Q39 / #806 original covers: a better priority passes an older ticket, and
/// a better waiter prevents a grant. Both execute real advance and full check.
pub(super) fn grant_cover(blocked: bool) -> (result: (World, Outbox, u64))
    ensures result.0.nodes@[0].pool == !blocked,
        result.0.pool_capacity == 1, result.2 == 1,
        result.0.nodes@[1].ticket == Some(0),
        result.0.nodes@[0].pool ==> result.0.nodes@[1].ticket.unwrap() < result.2,
        result.0.nodes@[0].pool ==> result.0.nodes@[0].ticket.is_none(),
        !result.0.nodes@[0].pool ==> result.0.nodes@[0].ticket == Some(1),
        world_checked(result.0),
{
    let world = pool_world(if blocked { Priority::P1 } else { Priority::P0 },
        if blocked { Priority::P0 } else { Priority::P1 }, Priority::Unranked,
        Some(1), Some(0), None, 2);
    proof {
        reveal(pool_fixture); reveal(pool_node); reveal(initial_node); reveal(best);
        assert(best(world, 0) == !blocked) by {
            if !blocked {
                assert forall|j: int| 0 <= j < 3 && j != 0
                    && !(world.nodes@[j].phase is Ready) && world.nodes@[j].ticket.is_some()
                    implies key_less(spec_key(world.nodes@[0].priority, 1),
                        spec_key(world.nodes@[j].priority, world.nodes@[j].ticket.unwrap())) by {
                    assert(j == 1);
                }
            }
        }
    }
    let mine = world.nodes[0].ticket.unwrap();
    let (world, out) = pool_grant(world);
    (world, out, mine)
}
}
