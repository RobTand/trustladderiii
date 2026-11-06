//! #693 / #806 W7: executable original one-root prerequisite constructors.
use super::port_deps_specs::*;
use super::*;

verus! {
/// #806: restore the three fields changed by the original waiting constructor.
pub(super) open spec fn original_node(n: Node) -> bool {
    initial_node(Node { phase: Phase::Ingesting, waiting_prerequisite: None,
        credit: Credit { ready: false, start: 0, earned: 0, added: 0, spent: 0,
            merge_credited: false, withheld: false }, ..n }, RepoId(0))
        && n.phase == Phase::Unplanned && n.credit == (Credit {
            ready: true, start: 4, earned: 0, added: 0, spent: 0,
            merge_credited: false, withheld: true })
}

/// #806: fixed original external prerequisite, with arbitrary observed status.
pub(super) open spec fn target() -> DependencyTarget {
    DependencyTarget { kind: PrerequisiteKind::Issue, number: 2761, request: None }
}

/// #806: exact original storage/capacities, keeping waiting-note identity free
/// after a handler. The constructor below sets the original number explicitly.
pub(super) open spec fn blocked(w: World, status: DependencyStatus) -> bool {
    w.nodes@.len() == 1 && original_node(w.nodes@[0]) && w.nodes@[0].waiting_prerequisite.is_some()
        && w.gates@ == seq![Gate { w: 1 }] && w.pool_capacity == 1
        && w.next_ticket == 0 && w.next_question == 0 && !w.fail_fast && w.file_sets@.len() == 0
        && w.dependencies@ == seq![Dependency { node: NodeId(0), target: target(), status, rejected: false }]
}

/// #806: original waiting worlds satisfy all production checker premises.
pub(super) proof fn blocked_is_checked(w: World, status: DependencyStatus)
    requires blocked(w, status),
    ensures checker_accepted(w), world_headroom(w), storage_ordered(w),
{
    reveal(initial_node); reveal(checker_accepted); reveal(retirement::node_rules);
    reveal(world_headroom); reveal(storage_ordered); reveal(footprint::ordered_table);
    reveal(dependencies::zero_credit);
    footprint::inline_is_valid(w, 0);
    reveal_with_fuel(open_prefix, 2); reveal_with_fuel(potential_prefix, 2);
    assert forall|i: int| 0 <= i < w.nodes@.len()
        implies retirement::node_rules(w, w.nodes@[i]) && !w.nodes@[i].holds by { assert(i == 0); }
}

/// State-table prerequisite wait / #806: the exact original single-root world,
/// constructed executably and accepted by the real complete checker.
pub(super) fn waiting_world() -> (world: World)
    ensures blocked(world, DependencyStatus::Pending), world.nodes@[0].waiting_prerequisite == Some(2761),
        world_checked(world), world_headroom(world), storage_ordered(world),
{
    let mut node = Node::new(RepoId(0));
    node.phase = Phase::Unplanned;
    node.credit.ready = true;
    node.credit.start = 4;
    node.credit.withheld = true;
    node.waiting_prerequisite = Some(2761);
    let mut nodes = Vec::new(); nodes.push(node);
    let mut gates = Vec::new(); gates.push(Gate { w: 1 });
    let mut dependencies = Vec::new();
    dependencies.push(Dependency { node: NodeId(0), target: DependencyTarget {
        kind: PrerequisiteKind::Issue, number: 2761, request: None },
        status: DependencyStatus::Pending, rejected: false });
    let world = World { nodes, gates, dependencies, pool_capacity: 1,
        next_ticket: 0, next_question: 0, fail_fast: false, file_sets: Vec::new() };
    proof { reveal(initial_node); assert(blocked(world, DependencyStatus::Pending));
        blocked_is_checked(world, DependencyStatus::Pending); reveal(port_deps_checker_accepted); }
    let accepted = world.check();
    assert(accepted.is_ok());
    world
}

/// #806 original waiting cover: real advance blocks with an empty footprint,
/// no resources, no output, zero balance, and complete checker acceptance.
pub(super) fn waiting_cover() -> (result: World)
    ensures blocked(result, DependencyStatus::Pending), result.nodes@[0].waiting_prerequisite.is_some(),
        !result.nodes@[0].holds && !result.nodes@[0].pool && result.nodes@[0].ticket.is_none()
            && result.nodes@[0].run.is_none() && footprint::footprint_empty(result.nodes@[0].footprint),
{
    let world = waiting_world();
    let (after, out, balance) = port_deps_terminal::waiting_holds_nothing(world, 0, 0);
    assert(out.len == 0 && balance == 0);
    proof { reveal(unchanged); reveal(initial_node); footprint::inline_is_valid(after, 0); blocked_is_checked(after, DependencyStatus::Pending);
        reveal(port_deps_checker_accepted); }
    let accepted = after.check();
    assert(accepted.is_ok());
    after
}

/// #806 original abandonment cover: real apply announces once, any later status
/// stutters silently, and real advance keeps the original root blocked forever.
pub(super) fn abandoned_cover(repeated_status: DependencyStatus) -> (result: (World, Outbox))
    ensures blocked(result.0, DependencyStatus::Abandoned),
        result.0.nodes@[0].waiting_prerequisite.is_some(),
        result.1.len == 1 && result.1.items@[0] == Some(Out::Notice {
            node: NodeId(0), notice: Notice::Note(NoteKind::DependencyAbandoned { number: 2761 }) }),
{
    let mut world = waiting_world();
    let ghost before = world;
    let mut out = Outbox::new();
    proof { reveal(empty_outbox); reveal(storage_ordered); checked_reserves(world); reveal(step_scope);
        reveal(blocked); reveal(target); reveal(pending_key); reveal(key);
        assert(world.dependencies@.len() == 1);
        assert(world.dependencies@[0].target == target());
        assert(key(world.dependencies@[0], 0, target()));
        assert forall|k: int| 0 <= k < world.dependencies@.len()
            && key(world.dependencies@[k], 0, target()) implies world.dependencies@[k].target == target()
                && !world.dependencies@[k].rejected
                && world.dependencies@[k].status == DependencyStatus::Pending by { assert(k == 0); }
        assert(pending_key(world, 0, target())); }
    apply(&mut world, Event::Fact { node: NodeId(0), fact: Fact::Dependency {
        target: DependencyTarget { kind: PrerequisiteKind::Issue, number: 2761, request: None },
        status: DependencyStatus::Abandoned } }, &mut out);
    proof {
        reveal(handler_post); reveal(resolved); reveal(dependencies::dependency_frame);
        reveal(recorded_output);
        assert(world.dependencies@.len() == 1);
        dependencies::prerequisite_definition(world, NodeId(0));
        dependencies::delivered_definition(world.dependencies@, NodeId(0));
        reveal(original_node); reveal(initial_node);
        assert(world.nodes@[0].credit.withheld);
        assert(original_node(world.nodes@[0]));
        assert(world.dependencies@ =~= seq![Dependency { node: NodeId(0), target: target(),
            status: DependencyStatus::Abandoned, rejected: false }]);
        assert(world.dependencies@[0].node == NodeId(0));
        assert(world.dependencies@[0].status == DependencyStatus::Abandoned);
        dependencies::delivered_definition(world.dependencies@, NodeId(0));
        assert(!dependencies::delivered(world.dependencies@, NodeId(0)));
        assert(world.nodes@[0].waiting_prerequisite.is_some());
        assert(blocked(world, DependencyStatus::Abandoned));
        blocked_is_checked(world, DependencyStatus::Abandoned); reveal(port_deps_checker_accepted);
    }
    let accepted = world.check();
    assert(accepted.is_ok());
    proof { reveal(storage_ordered); checked_reserves(world); reveal(step_scope); }
    let (world, repeated_out) = port_deps_terminal::terminal_stutters(world, 0,
        DependencyTarget { kind: PrerequisiteKind::Issue, number: 2761, request: None }, repeated_status);
    assert(repeated_out.len == 0);
    proof { reveal(unchanged); blocked_is_checked(world, DependencyStatus::Abandoned);
        reveal(port_deps_checker_accepted); }
    let accepted = world.check();
    assert(accepted.is_ok());
    let (world, waiting_out, balance) = port_deps_terminal::waiting_holds_nothing(world, 0, 0);
    assert(waiting_out.len == 0 && balance == 0);
    proof { reveal(unchanged); blocked_is_checked(world, DependencyStatus::Abandoned);
        reveal(port_deps_checker_accepted); }
    let accepted = world.check();
    assert(accepted.is_ok());
    (world, out)
}

/// #806: the original root after delivery, before the separate admission move.
pub(super) open spec fn ready_for_admission(w: World) -> bool {
    w.nodes@.len() == 1 && original_node(w.nodes@[0])
        && w.nodes@[0].waiting_prerequisite.is_none()
        && w.gates@ == seq![Gate { w: 1 }] && w.pool_capacity == 1
        && w.next_ticket == 0 && w.next_question == 0 && !w.fail_fast && w.file_sets@.len() == 0
        && w.dependencies@ == seq![Dependency { node: NodeId(0), target: target(),
            status: DependencyStatus::Delivered, rejected: false }]
}

/// #806 original delivery assertion: one Unplanned root's executable measure
/// fits u64. Other dependency theorems use the unbounded natural measure.
pub(super) proof fn fixture_measure(w: World)
    requires w.nodes@.len() == 1, original_node(w.nodes@[0]), w.dependencies@.len() == 1,
        w.dependencies@[0].node == NodeId(0),
    ensures progress_measure(w) <= u64::MAX,
        progress_measure(w) == 1794 + if w.dependencies@[0].status == DependencyStatus::Delivered { 0nat } else { 1nat },
{
    reveal(initial_node); reveal(distance); reveal_with_fuel(node_total, 2);
    progress::measure_definition(w, 1);
    assert(node_total(w.nodes@) == 1794);
}

/// #806 original delivery witness: run real apply, compare both real machine
/// measures, expose the exact single-row update, and run the complete checker.
pub(super) fn delivery_fact() -> (result: (World, Outbox, u64, u64))
    ensures ready_for_admission(result.0), world_checked(result.0),
        world_headroom(result.0), storage_ordered(result.0),
        result.1.len == 0, result.3 < result.2,
{
    let mut world = waiting_world();
    proof { fixture_measure(world); world_checked_definition(world); }
    let before_measure = world.measure();
    let ghost before = world;
    let mut out = Outbox::new();
    proof {
        reveal(blocked); reveal(target); reveal(pending_key); reveal(key);
        assert(key(world.dependencies@[0], 0, target()));
        assert forall|k: int| 0 <= k < world.dependencies@.len()
            && key(world.dependencies@[k], 0, target()) implies world.dependencies@[k].target == target()
                && !world.dependencies@[k].rejected
                && world.dependencies@[k].status == DependencyStatus::Pending by { assert(k == 0); }
        assert(pending_key(world, 0, target()));
        reveal(empty_outbox); reveal(storage_ordered); checked_reserves(world); reveal(step_scope);
    }
    apply(&mut world, Event::Fact { node: NodeId(0), fact: Fact::Dependency {
        target: DependencyTarget { kind: PrerequisiteKind::Issue, number: 2761, request: None },
        status: DependencyStatus::Delivered } }, &mut out);
    proof {
        reveal(handler_post); reveal(resolved); reveal(dependencies::dependency_frame);
        reveal(recorded_output);
        dependencies::prerequisite_definition(world, NodeId(0));
        dependencies::delivered_definition(world.dependencies@, NodeId(0));
        reveal(ready_for_admission); reveal(original_node); reveal(initial_node);
        assert(world.nodes@[0].credit.withheld);
        assert(original_node(world.nodes@[0]));
        assert(world.nodes@[0].waiting_prerequisite.is_none());
        assert(world.dependencies@ =~= seq![Dependency { node: NodeId(0), target: target(),
            status: DependencyStatus::Delivered, rejected: false }]);
        assert(ready_for_admission(world));
        fixture_measure(world);
        reveal(checker_accepted); reveal(retirement::node_rules); reveal(initial_node);
        reveal(dependencies::zero_credit); reveal(world_headroom); reveal(storage_ordered);
        reveal(footprint::ordered_table); footprint::inline_is_valid(world, 0);
        reveal_with_fuel(open_prefix, 2); reveal_with_fuel(potential_prefix, 2);
        assert forall|i: int| 0 <= i < world.nodes@.len()
            implies retirement::node_rules(world, world.nodes@[i]) && !world.nodes@[i].holds by { assert(i == 0); }
        assert(checker_accepted(world)); reveal(port_deps_checker_accepted);
    }
    let after_measure = world.measure();
    assert(after_measure < before_measure);
    let accepted = world.check();
    assert(accepted.is_ok());
    (world, out, before_measure, after_measure)
}
}
