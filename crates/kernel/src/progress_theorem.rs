//! Q30/Q39 / #805: global theorem and constructive scope witness.
use super::*;
verus! {
/// State-table prerequisite wait / #805: a real constructor supplies a settled
/// open world in scope, whose selected event is prerequisite delivery. This is
/// a reachability witness, not a finite bound on the universal theorem.
fn progress_scope_witness() -> (world: World)
    ensures progress_scope(world),
        dependencies::delivery_input(world, 0, DependencyTarget {
            kind: PrerequisiteKind::Issue, number: 1, request: None }),
{
    let mut node = Node::new(RepoId(0));
    node.phase = Phase::Unplanned;
    node.credit.ready = true;
    node.credit.start = 3;
    node.credit.withheld = true;
    node.planning_started = false;
    let mut nodes = Vec::new();
    nodes.push(node);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    let target = DependencyTarget { kind: PrerequisiteKind::Issue, number: 1, request: None };
    let mut dependencies = Vec::new();
    dependencies.push(Dependency { node: NodeId(0), target, status: DependencyStatus::Pending, rejected: false });
    let world = World { nodes, gates, dependencies, pool_capacity: 1,
        next_ticket: 0, next_question: 0, file_sets: Vec::new(), fail_fast: false };
    proof {
        reveal(initial_node);
        reveal(progress_scope);
        reveal(world_checked);
        reveal(settled_world);
        reveal(storage_ordered);
        reveal(world_headroom);
        reveal_with_fuel(open_prefix, 2);
        assert(open_prefix(world.nodes@, world.nodes@.len()) == 1);
        reveal(fresh_commands);
        reveal(dependencies::delivery_input);
        reveal(footprint::ordered_table);
        dependencies::indexable_definition(world);
        dependencies::pending_blocks(world, 0);
        footprint::inline_is_valid(world, 0);
        assert(world_checked(world));
        assert(settled_world(world));
        assert(world_headroom(world));
        assert(storage_ordered(world));
        assert(fresh_commands(world));
    }
    world
}

/// Q30/Q39: an executable outside witness strictly lowers the natural rank
/// through real step and settle. The theorem covers arbitrary finite storage;
/// counter reserves are exactly the enforced #771 premises, with fresh IDs.
fn kernel_progress(world: World) -> (result: (World, Vec<Out>))
    requires progress_scope(world),
    ensures progress_measure(result.0) < progress_measure(world),
        step_budget(world, result.0),
{
    proof {
        reveal(progress_scope);
        reveal(progress_input);
        reveal(storage_ordered);
        checked_reserves(world);
        outside_branch_exists(world);
    }
    let event = world.witness();
    proof { reveal(witness_selection); }
    step(world, event.unwrap())
}

}
