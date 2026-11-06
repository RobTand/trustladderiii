//! #693 / #806 W7: source-connected cycle preservation and original covers.
use super::port_deps_graph::*;
use super::port_deps_specs::*;
use super::*;
use vstd::iset::ISet;

verus! {
/// State-table cycle rejection / #806: real apply preserves graph acyclicity
/// for arbitrary declaration counts in production's <=64-root bitset branch.
pub(super) fn register_preserves(mut world: World, i: usize, target: DependencyTarget)
    -> (result: (World, Outbox))
    requires step_scope(world), world.nodes@.len() <= 64, acyclic(world),
        i < world.nodes@.len(), i <= u64::MAX as int,
    ensures acyclic(result.0), dependencies::edges_indexable(result.0),
        dependencies::dependency_frame(world, result.0, i as int),
        new_key(world, i as int, target) ==> result.0.dependencies@ == world.dependencies@.push(Dependency {
            node: NodeId(i as u64), target, status: DependencyStatus::Pending,
            rejected: cycle_rejected(world, i as int, target, DependencyStatus::Pending) }),
{
    let ghost before = world;
    let mut out = Outbox::new();
    proof { reveal(step_scope); reveal(empty_outbox); }
    apply(&mut world, Event::Fact { node: NodeId(i as u64), fact: Fact::Dependency {
        target, status: DependencyStatus::Pending } }, &mut out);
    proof { reveal(handler_post); reveal(dependencies::dependency_frame); }
    let checked = world.dependency_graph_acyclic();
    proof { reveal(port_deps_graph_result); }
    assert(checked);
    (world, out)
}

/// #806: original three-root, two-Pending-declaration chain, with every node
/// built by Node::new. The original symbolic fixture includes this construction.
pub(super) open spec fn chain(w: World) -> bool {
    w.nodes@.len() == 3 && (forall|i: int| 0 <= i < 3 ==> initial_node(w.nodes@[i], RepoId(0)))
        && w.gates@ == seq![Gate { w: 3 }] && w.pool_capacity == 3
        && w.next_ticket == 0 && w.next_question == 0 && w.file_sets@.len() == 0 && !w.fail_fast
        && w.dependencies@ == seq![
            Dependency { node: NodeId(0), target: DependencyTarget {
                kind: PrerequisiteKind::Issue, number: 1, request: Some(NodeId(1)) },
                status: DependencyStatus::Pending, rejected: false },
            Dependency { node: NodeId(1), target: DependencyTarget {
                kind: PrerequisiteKind::Issue, number: 2, request: Some(NodeId(2)) },
                status: DependencyStatus::Pending, rejected: false }]
}

/// #806 original no-cycle assumptions: all three roots are freshly constructed,
/// and both prior endpoints are arbitrary valid indices. No checker or graph
/// shape assumption is added by the execution-scope bridge below.
pub(super) open spec fn original_graph(w: World) -> bool {
    w.nodes@.len() == 3
        && (forall|i: int| 0 <= i < 3 ==> initial_node(w.nodes@[i], RepoId(0)))
        && w.gates@ == seq![Gate { w: 3 }] && w.pool_capacity == 3
        && w.next_ticket == 0 && w.next_question == 0 && w.file_sets@.len() == 0 && !w.fail_fast
        && w.dependencies@.len() == 2
        && (forall|k: int| 0 <= k < 2 ==> w.dependencies@[k].node.0 < 3
            && w.dependencies@[k].target.request.is_some()
            && w.dependencies@[k].target.request.unwrap().0 < 3
            && w.dependencies@[k].target.kind == PrerequisiteKind::Issue
            && w.dependencies@[k].target.number == k + 1
            && w.dependencies@[k].status == DependencyStatus::Pending && !w.dependencies@[k].rejected)
}

/// #806: every original symbolic cycle fixture meets real apply's existing
/// machine-safety premises. Endpoint choices and prior acyclicity stay free.
pub(super) proof fn original_graph_scope(w: World)
    requires original_graph(w),
    ensures step_scope(w),
{
    reveal(initial_node); reveal(step_scope); reveal(world_headroom);
    reveal(footprint::ordered_table); reveal_with_fuel(open_prefix, 4);
    footprint::inline_is_valid(w, 0); dependencies::indexable_definition(w);
    assert forall|i: int| 0 <= i < w.nodes@.len() implies arithmetic_consistent(w.nodes@[i])
        && settle_shape(w.nodes@[i]) && stopping_effect_fits(w.nodes@[i])
        && footprint::valid_footprint(w, w.nodes@[i].footprint) by { assert(i == 0 || i == 1 || i == 2); }
}

/// #806: explicit cuts show the original chain is acyclic, without replacing
/// the production traversal. These cuts also classify both new-edge covers.
pub(super) proof fn chain_is_acyclic(w: World)
    requires chain(w),
    ensures acyclic(w), !reaches(w, NodeId(2), NodeId(0)), reaches(w, NodeId(0), NodeId(0)),
{
    reveal(reaches);
    let above_zero = ISet::new(|n: NodeId| n.0 > 0);
    let above_one = ISet::new(|n: NodeId| n.0 > 1);
    assert(closed(w, above_zero));
    assert(closed(w, above_one));
    assert forall|k: int| 0 <= k < w.dependencies@.len() && active(w, w.dependencies@[k])
        implies !reaches(w, w.dependencies@[k].target.request.unwrap(), w.dependencies@[k].node) by {
        assert(k == 0 || k == 1);
    }
}

/// #806 cycle covers: executable original-domain constructor and real graph
/// query, retaining the original three-root bound solely for witness fidelity.
pub(super) fn chain_world() -> (world: World)
    ensures chain(world), acyclic(world), step_scope(world),
{
    let mut nodes = Vec::new();
    nodes.push(Node::new(RepoId(0)));
    nodes.push(Node::new(RepoId(0)));
    nodes.push(Node::new(RepoId(0)));
    let mut gates = Vec::new(); gates.push(Gate { w: 3 });
    let mut dependencies = Vec::new();
    dependencies.push(Dependency { node: NodeId(0), target: DependencyTarget {
        kind: PrerequisiteKind::Issue, number: 1, request: Some(NodeId(1)) },
        status: DependencyStatus::Pending, rejected: false });
    dependencies.push(Dependency { node: NodeId(1), target: DependencyTarget {
        kind: PrerequisiteKind::Issue, number: 2, request: Some(NodeId(2)) },
        status: DependencyStatus::Pending, rejected: false });
    let world = World { nodes, gates, dependencies, pool_capacity: 3, next_ticket: 0,
        next_question: 0, file_sets: Vec::new(), fail_fast: false };
    proof {
        reveal(chain);
        assert forall|i: int| 0 <= i < 3 implies initial_node(world.nodes@[i], RepoId(0)) by {
            assert(i == 0 || i == 1 || i == 2);
        }
        assert(world.gates@ =~= seq![Gate { w: 3 }]);
        assert(world.dependencies@ =~= seq![
            Dependency { node: NodeId(0), target: DependencyTarget {
                kind: PrerequisiteKind::Issue, number: 1, request: Some(NodeId(1)) },
                status: DependencyStatus::Pending, rejected: false },
            Dependency { node: NodeId(1), target: DependencyTarget {
                kind: PrerequisiteKind::Issue, number: 2, request: Some(NodeId(2)) },
                status: DependencyStatus::Pending, rejected: false }]);
        assert(chain(world)); chain_is_acyclic(world);
        reveal(initial_node); reveal(step_scope); reveal(world_headroom);
        reveal(footprint::ordered_table); reveal_with_fuel(open_prefix, 4);
        footprint::inline_is_valid(world, 0);
        dependencies::indexable_definition(world);
        assert forall|i: int| 0 <= i < world.nodes@.len() implies arithmetic_consistent(world.nodes@[i])
            && settle_shape(world.nodes@[i]) && stopping_effect_fits(world.nodes@[i])
            && footprint::valid_footprint(world, world.nodes@[i].footprint) by { assert(i == 0 || i == 1 || i == 2); }
    }
    let acyclic = world.dependency_graph_acyclic();
    proof { reveal(port_deps_graph_result); }
    assert(acyclic);
    world
}

/// #806 original cycle covers: a self-cycle is rejected; the forward shortcut
/// is accepted. Both execute real apply and leave a real acyclic graph.
pub(super) fn cycle_cover(rejected: bool) -> (result: World)
    ensures result.nodes@.len() == 3, result.dependencies@.len() == 3,
        result.dependencies@[2].rejected == rejected, acyclic(result),
{
    let world = chain_world();
    let target = DependencyTarget { kind: PrerequisiteKind::Issue, number: 3,
        request: Some(NodeId(if rejected { 0 } else { 2 })) };
    proof { reveal(initial_node); chain_is_acyclic(world); assert(new_key(world, 0, target)); }
    let (world, out) = register_preserves(world, 0, target);
    proof { reveal(dependencies::dependency_frame); }
    world
}
}
