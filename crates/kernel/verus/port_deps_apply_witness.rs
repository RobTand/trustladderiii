//! #806 W7: executable original-domain covers for external dependency apply.
use super::port_deps_single::{accepted, dependencies_ok, domain};
use super::*;

verus! {
/// #806 original dependency-fact covers: closed, ingesting, maker and queued roots.
pub(super) fn fact_world(case: u8) -> (w: World)
    requires case < 4,
    ensures domain(w), accepted(w), w.dependencies@.len() == 1,
        w.dependencies@[0].target == (DependencyTarget { kind: PrerequisiteKind::Issue, number: 2761, request: None }),
        w.dependencies@[0].status == if case < 2 { DependencyStatus::Pending } else { DependencyStatus::Delivered },
        w.nodes@[0].phase == match case { 0 => Phase::Closed(Outcome::Stopped),
            1 => Phase::Ingesting, 2 => Phase::Making, _ => Phase::Queued },
{
    let mut n = Node::new(RepoId(0));
    n.phase = match case { 0 => Phase::Closed(Outcome::Stopped),
        1 => Phase::Ingesting, 2 => Phase::Making, _ => Phase::Queued };
    if case >= 2 {
        n.credit.ready = true; n.credit.start = 4; n.attempt = 1;
        n.footprint = Footprint::inline(1); n.holds = true;
    }
    if case == 3 {
        n.head = Some(HeadId(1)); n.checked = n.head; n.approved = n.head;
        n.had_pr = true; n.pr_open = true; n.worktree = true;
    }
    let mut nodes = Vec::new(); nodes.push(n);
    let mut gates = Vec::new(); gates.push(Gate { w: 3 });
    let mut w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    proof {
        reveal(initial_node); reveal(domain); reveal(accepted); reveal(dependencies_ok);
        reveal(retirement::node_rules); reveal(dependencies::declarations_unique);
        footprint::inline_is_valid(w, 0); footprint::inline_is_valid(w, 1); port_deps_apply::scope(w);
    }
    let checked = w.check(); assert(checked.is_ok());
    let ghost before_edges = w;
    w.dependencies.push(Dependency { node: NodeId(0), target: DependencyTarget {
        kind: PrerequisiteKind::Issue, number: 2761, request: None },
        status: if case < 2 { DependencyStatus::Pending } else { DependencyStatus::Delivered }, rejected: false });
    proof {
        reveal(domain); reveal(accepted); reveal(dependencies_ok); reveal(dependencies::declarations_unique);
        reveal(dependencies::zero_credit); dependencies::prerequisite_definition(w, NodeId(0));
        dependencies::delivered_definition(w.dependencies@, NodeId(0));
        footprint::inline_is_valid(w, 0); footprint::inline_is_valid(w, 1);
        retirement::node_frame(before_edges, w, w.nodes@[0]);
        assert(retirement::node_rules(w, w.nodes@[0]));
        assert(dependencies_ok(w));
        port_deps_apply::scope(w);
    }
    let checked = w.check(); assert(checked.is_ok());
    w
}

/// #806 apply_dependency covers: run real apply with case-specific terminal observations.
pub(super) fn fact_cover(case: u8) -> (result: (World, Outbox))
    requires case < 4,
    ensures accepted(result.0), result.0.nodes@[0].phase == match case {
        0 => Phase::Closed(Outcome::Stopped), 1 => Phase::Ingesting,
        2 => Phase::Making, _ => Phase::Queued },
        case == 0 ==> result.0.dependencies@[0].status == DependencyStatus::Delivered,
        case == 1 ==> result.0.dependencies@[0].status == DependencyStatus::Abandoned,
{
    let w = fact_world(case);
    port_deps_apply::dependency_fact(w, if case == 0 { DependencyStatus::Delivered } else { DependencyStatus::Abandoned })
}

/// #806 original registration covers: arbitrary Node domain includes both values of planning_started.
pub(super) fn registration_world(started: bool) -> (w: World)
    ensures domain(w), accepted(w), w.dependencies@.len() == 0,
        w.nodes@[0].planning_started == started, w.nodes@[0].phase == Phase::Ingesting,
{
    let mut n = Node::new(RepoId(0)); n.planning_started = started;
    let mut nodes = Vec::new(); nodes.push(n);
    let mut gates = Vec::new(); gates.push(Gate { w: 3 });
    let w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    proof {
        reveal(initial_node); reveal(domain); reveal(accepted); reveal(dependencies_ok);
        reveal(retirement::node_rules); reveal(dependencies::declarations_unique);
        footprint::inline_is_valid(w, 0); footprint::inline_is_valid(w, 1); port_deps_apply::scope(w);
    }
    let checked = w.check(); assert(checked.is_ok());
    w
}

/// #806 apply_dependency_registration covers: real apply refuses started roots and admits unstarted roots.
pub(super) fn registration_cover(started: bool) -> (result: (World, Outbox))
    ensures accepted(result.0), result.0.nodes@[0].planning_started == started,
        result.0.dependencies@.len() == if started { 0nat } else { 1nat },
{
    let w = registration_world(started);
    let (w, out) = port_deps_apply::registration(w, DependencyStatus::Pending);
    (w, out)
}
}
