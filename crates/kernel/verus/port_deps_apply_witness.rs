//! #806 W7: executable original-domain covers for external dependency apply.
use super::port_deps_single::{accepted, dependencies_ok, domain};
use super::*;

verus! {
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
