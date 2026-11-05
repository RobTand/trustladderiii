//! #806 B1: executable original-domain covers for all five Unplanned harnesses.
use super::progress_unplanned::*;
use super::*;

verus! {
/// #693 / #806: finite fixtures have no active graph edge, including rejected cycles.
pub(super) open spec fn fixture_edges(w: World) -> bool {
    w.nodes@.len() == 2 && w.nodes@[0].phase == Phase::Unplanned
        && w.nodes@[0].question.is_none() && !w.nodes@[0].planning_started
        && (forall|i: int| 0 <= i < w.dependencies@.len() ==> {
            let d = #[trigger] w.dependencies@[i];
            d.node == NodeId(0) && d.target.number > 0
                && (d.target.request.is_none() || (d.rejected && d.target.request == Some(NodeId(0))))
                && dependencies::dependency_checked(w, d)
        })
        && dependencies::declarations_unique(w)
        && (w.dependencies@.len() == 0 ==> !w.nodes@[0].credit.withheld)
        && !w.nodes@[1].credit.withheld
}

/// #806: complete checker domain for these two-node constructive witnesses.
pub(super) open spec fn check_domain(w: World) -> bool {
    w.nodes@.len() == 2 && w.gates@.len() == 2 && fixture_edges(w)
}

/// #806: the bounded original construction, with complete node/gate/pool checks.
pub(super) open spec fn original_domain(w: World) -> bool {
    check_domain(w) && retirement::accepted(w) && w.file_sets@.len() == 0
        && w.pool_capacity <= 2 && w.next_ticket < 1099511627776u64 && w.next_question < 1099511627776u64
        && (forall|i: int| 0 <= i < 2 ==> {
            let n = #[trigger] w.nodes@[i];
            n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
                && n.last_cmd < 1099511627776u64 && n.repo.0 < 2
                && n.proposed.is_none() && n.head.is_none() && n.checked.is_none()
                && n.approved.is_none() && n.merged.is_none()
                && n.credit.start < 1073741824u64 && n.credit.earned < 1073741824u64
                && n.credit.added < 1073741824u64 && n.credit.spent < 1073741824u64
                && (match n.ticket { None => true, Some(t) => t < 1099511627776u64 })
                && n.run.is_none() && n.job.is_none() && n.question.is_none()
                && (match n.effect { None => true, Some(e) => e.seq < 1099511627776u64 })
        })
        && (forall|r: int| 0 <= r < 2 ==> w.gates@[r].w <= 3)
}

/// State-table Unplanned covers: 0 Resume, 1 Ack, 2 incident, 3 admission,
/// 4 other gate occupant, 5 Pending, 6 Abandoned, 7 rejected self-cycle.
/// Edge count 0, 1, or 2 is exactly the retired constructor's finite domain.
pub(super) fn cover_world(case: u8, count: usize) -> (world: World)
    requires case < 8, count <= 2, case >= 5 ==> count > 0,
    ensures original_domain(world), scope(world), world.dependencies@.len() == count,
        case < 3 ==> obstructed(world.nodes@[0]) && world.nodes@[1].phase is Closed && settled_world(world),
        case == 0 ==> world.nodes@[0].paused && world.nodes@[0].effect.is_some() && world.nodes@[0].waiting_incident,
        case == 1 ==> !world.nodes@[0].paused && world.nodes@[0].effect.is_some() && world.nodes@[0].waiting_incident,
        case == 2 ==> !world.nodes@[0].paused && world.nodes@[0].effect.is_none() && world.nodes@[0].waiting_incident,
        case < 5 ==> dependencies::delivered(world.dependencies@, NodeId(0)),
        case >= 3 ==> !obstructed(world.nodes@[0]),
        case == 3 ==> movable(world, 0), case == 4 ==> !movable(world, 0),
        3 <= case <= 4 ==> (slot_waiters(world.nodes@, world.nodes@.len(), world.nodes@[0].repo)
            < world.gates@[world.nodes@[0].repo.0 as int].w) == (case == 3),
        case >= 5 ==> world.dependencies@[0].node == NodeId(0)
            && (world.dependencies@[0].rejected || world.dependencies@[0].status != DependencyStatus::Delivered),
        case == 5 ==> world.dependencies@[0].status == DependencyStatus::Pending && !world.dependencies@[0].rejected,
        case == 6 ==> world.dependencies@[0].status == DependencyStatus::Abandoned,
        case == 7 ==> world.dependencies@[0].rejected && world.dependencies@[0].target.request == Some(NodeId(0)),
{
    let mut first = Node::new(RepoId(0));
    first.phase = Phase::Unplanned;
    first.credit.ready = true;
    first.credit.start = 4;
    first.credit.withheld = case >= 5;
    first.paused = case == 0;
    first.waiting_incident = case < 3;
    first.incident_needs_rob = case < 3;
    if case < 2 {
        first.next_seq = 1;
        first.effect = Some(InFlight { seq: 0, kind: EffectKind::ClosePr, cancelling: true });
    }
    let mut second = Node::new(RepoId(0));
    second.phase = Phase::Closed(Outcome::Stopped);
    if case == 4 {
        second.phase = Phase::Ready(Resume::Making);
        second.credit.ready = true;
        second.credit.start = 4;
        second.attempt = 1;
        second.footprint = Footprint::inline(1);
        second.ticket = Some(0);
    }
    let mut nodes = Vec::new(); nodes.push(first); nodes.push(second);
    let mut gates = Vec::new(); gates.push(Gate { w: 1 }); gates.push(Gate { w: 1 });
    let mut dependencies = Vec::new();
    let mut k = 0usize;
    while k < count
        invariant k <= count <= 2, dependencies@.len() == k,
            forall|j: int| 0 <= j < k ==> dependencies@[j] == (Dependency {
                node: NodeId(0), target: DependencyTarget { kind: PrerequisiteKind::Issue,
                    number: (j + 1) as u64, request: if case == 7 { Some(NodeId(0)) } else { None } },
                status: if case == 5 || case == 7 { DependencyStatus::Pending }
                    else if case == 6 { DependencyStatus::Abandoned } else { DependencyStatus::Delivered },
                rejected: case == 7 }),
        decreases count - k,
    {
        dependencies.push(Dependency { node: NodeId(0), target: DependencyTarget {
            kind: PrerequisiteKind::Issue, number: k as u64 + 1,
            request: if case == 7 { Some(NodeId(0)) } else { None } },
            status: if case == 5 || case == 7 { DependencyStatus::Pending }
                else if case == 6 { DependencyStatus::Abandoned } else { DependencyStatus::Delivered },
            rejected: case == 7 });
        k += 1;
    }
    let world = World { nodes, gates, dependencies, pool_capacity: 1, next_ticket: 1,
        next_question: 0, file_sets: Vec::new(), fail_fast: false };
    proof {
        reveal(initial_node); reveal(retirement::accepted); reveal(retirement::node_rules);
        reveal(storage_ordered); reveal(footprint::ordered_table);
        dependencies::delivered_definition(world.dependencies@, NodeId(0));
        dependencies::prerequisite_definition(world, NodeId(0));
        footprint::inline_is_valid(world, 0); footprint::inline_is_valid(world, 1);
        assert forall|i: int| 0 <= i < 2 implies retirement::node_rules(world, world.nodes@[i]) by {
            assert(i == 0 || i == 1);
        }
        assert forall|a: int, b: int| 0 <= a < b < world.dependencies@.len()
            implies !dependencies::same_declaration(world.dependencies@[a], world.dependencies@[b]) by {}
        reveal(dependencies::zero_credit);
        assert(fixture_edges(world));
        assert(original_domain(world));
        reveal(world_checked); dependencies::indexable_definition(world);
        original_bounds_supply_reserves(world);
        assert(scope(world));
        reveal(settled_world); reveal(movable); reveal(oldest_waiter);
        reveal_with_fuel(potential_prefix, 3);
        if case < 3 {
            assert forall|i: int| 0 <= i < 2 implies !movable(world, i) by { assert(i == 0 || i == 1); }
        }
    }
    proof { reveal(unplanned_check_domain); reveal(retirement_accepted); }
    let accepted = world.check();
    assert(accepted.is_ok());
    world
}

/// #806: every original cover is exported after real selector/apply or advance.
/// Tuple fields: before snapshot, actual successor, selected event, admitted,
/// gate occupant moved, waiting outbox length, and real waiting credit balance.
fn run_cover(case: u8, count: usize) -> (result: (Ghost<World>, World, Option<Event>, bool, bool, usize, i128))
    requires case < 8, count <= 2, case >= 5 ==> count > 0,
    ensures original_domain(result.0@), scope(result.0@),
        result.0@.dependencies@.len() == count,
        case < 3 ==> settled_world(result.0@) && result.0@.nodes@[1].phase is Closed
            && obstructed(result.0@.nodes@[0]) && result.2.is_some()
            && result.2 == Some(outside_event(result.0@.nodes@[0]))
            && witness_selection(result.0@, result.2)
            && result.1.nodes@[1] == result.0@.nodes@[1]
            && progress_measure(result.1) < progress_measure(result.0@),
        case == 0 ==> result.0@.nodes@[0].paused && result.0@.nodes@[0].effect.is_some()
            && result.0@.nodes@[0].waiting_incident,
        case == 1 ==> !result.0@.nodes@[0].paused && result.0@.nodes@[0].effect.is_some()
            && result.0@.nodes@[0].waiting_incident,
        case == 2 ==> !result.0@.nodes@[0].paused && result.0@.nodes@[0].effect.is_none()
            && result.0@.nodes@[0].waiting_incident,
        case < 5 ==> dependencies::delivered(result.0@.dependencies@, NodeId(0)),
        case >= 3 ==> !obstructed(result.0@.nodes@[0]) && result.2.is_none(),
        result.3 == (case == 3), result.4 == (case == 4),
        case == 3 ==> result.1.nodes@[0].phase == (Phase::Planning { must_split: false })
            && result.1.nodes@[1] == result.0@.nodes@[1]
            && progress_measure(result.1) < progress_measure(result.0@),
        case == 4 ==> result.0@.nodes@[1].phase is Ready
            && result.0@.nodes@[1].ticket.is_some(),
        case >= 5 ==> progress_unplanned_waiting::unchanged(result.0@, result.1)
            && !result.1.nodes@[0].holds && !result.1.nodes@[0].pool
            && result.1.nodes@[0].ticket.is_none() && result.1.nodes@[0].run.is_none()
            && result.5 == 0 && result.6 == 0,
        case == 5 ==> result.0@.dependencies@[0].status == DependencyStatus::Pending
            && !result.0@.dependencies@[0].rejected,
        case == 6 ==> result.0@.dependencies@[0].status == DependencyStatus::Abandoned,
        case == 7 ==> result.0@.dependencies@[0].rejected
            && result.0@.dependencies@[0].target.request == Some(NodeId(0)),
{
    let world = cover_world(case, count);
    let ghost before = world;
    if case < 3 {
        let (after, event) = verify_outside(world);
        (Ghost(before), after, Some(event), false, false, 0, 0)
    } else if case < 5 {
        let (after, admitted, occupant_moved) = progress_unplanned_admission::verify_admission(world);
        proof { reveal_with_fuel(potential_prefix, 3); }
        assert(admitted == (case == 3));
        (Ghost(before), after, None, admitted, occupant_moved, 0, 0)
    } else {
        let (after, admitted, output_len, balance) = progress_unplanned_waiting::verify_waiting(world, 0);
        (Ghost(before), after, None, admitted, false, output_len, balance)
    }
}
}
