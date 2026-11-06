//! #806 W7: original-domain real scheduler composition.
use super::port_deps_specs::*;
use super::port_deps_witness::*;
use super::*;

verus! {
/// #693 / #806 original delivery cover: real apply decreases the real measure,
/// real admission succeeds, releases credit, and passes the complete checker.
pub(super) fn delivery_cover() -> (result: (World, Outbox, i128, u64, u64))
    ensures result.0.nodes@.len() == 1,
        result.0.nodes@[0].phase == (Phase::Planning { must_split: false }),
        result.0.nodes@[0].planning_started, !result.0.nodes@[0].credit.withheld,
        result.2 == 4, result.1.len == 0, result.4 < result.3,
        world_checked(result.0),
{
    let (mut world, mut out, before_measure, after_measure) = delivery_fact();
    proof {
        reveal(ready_for_admission); reveal(original_node); reveal(initial_node);
        reveal(storage_ordered); checked_reserves(world); reveal(settle_ready);
        potential_member(world.nodes@, 0, world.nodes@.len(), |n: Node| ticket_capacity(n));
        dependencies::delivered_definition(world.dependencies@, NodeId(0));
        reveal_with_fuel(potential_prefix, 2);
    }
    let moved = advance(&mut world, 0, &mut out);
    assert(moved);
    assert(world.nodes@[0].planning_started);
    assert(world.nodes@[0].phase == (Phase::Planning { must_split: false }));
    let balance = world.nodes[0].credit.balance();
    assert(balance == 4);
    proof {
        reveal(checker_accepted); reveal(retirement::node_rules); reveal(node_checked);
        reveal(dependencies::zero_credit); reveal(world_headroom); reveal(storage_ordered);
        reveal(footprint::ordered_table); footprint::inline_is_valid(world, 0);
        reveal_with_fuel(open_prefix, 2); reveal_with_fuel(potential_prefix, 2);
        assert forall|i: int| 0 <= i < world.nodes@.len()
            implies retirement::node_rules(world, world.nodes@[i]) && !world.nodes@[i].holds
            by { assert(i == 0); }
        assert(checker_accepted(world)); reveal(port_deps_checker_accepted);
    }
    let accepted = world.check();
    assert(accepted.is_ok());
    (world, out, balance, before_measure, after_measure)
}
}
