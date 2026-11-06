//! #806 W7: real stable prerequisite removal and preservation of terminal rows.
use super::port_deps_single::{accepted, dependencies_ok, domain};
use super::*;

verus! {
/// #806 original removal table: one Pending row and one distinct terminal row.
pub(super) open spec fn table(w: World) -> bool {
    w.dependencies@.len() == 2
        && w.dependencies@[0] == (Dependency { node: NodeId(0),
            target: DependencyTarget { kind: PrerequisiteKind::Issue, number: 2761, request: None },
            status: DependencyStatus::Pending, rejected: false })
        && w.dependencies@[1].node == NodeId(0)
        && w.dependencies@[1].target == (DependencyTarget { kind: PrerequisiteKind::Issue, number: 2762, request: None })
        && !w.dependencies@[1].rejected && w.dependencies@[1].status != DependencyStatus::Pending
}

/// #806: removing the changed declaration preserves every terminal outcome and checker rule.
pub(super) proof fn preserves(w: World, v: World)
    requires domain(w), accepted(w), table(w),
        port_deps_apply_specs::removed(w, v, 0, PrerequisiteKind::Issue, 2761),
    ensures domain(v), accepted(v),
        v.dependencies@.len() == if w.nodes@[0].phase == Phase::Ingesting { 1nat } else { 2nat },
        v.dependencies@[v.dependencies@.len() - 1] == w.dependencies@[1],
{
    reveal(accepted); reveal(domain); reveal(table); reveal(dependencies_ok);
    reveal(dependencies::declarations_unique); reveal(port_deps_apply_specs::removed);
    reveal(dependencies::dependency_frame); reveal(port_deps_specs::unchanged);
    reveal_with_fuel(port_deps_apply_specs::kept, 3);
    reveal(retirement::node_rules); reveal(dependencies::zero_credit);
    dependencies::prerequisite_definition(w, NodeId(0)); dependencies::prerequisite_definition(v, NodeId(0));
    dependencies::delivered_definition(w.dependencies@, NodeId(0));
    dependencies::delivered_definition(v.dependencies@, NodeId(0));
    footprint::storage_frame(w, v, w.nodes@[0].footprint, w.nodes@[0].footprint);
}

/// #806 apply_dependency_remove: real apply, exact retained row and complete checker acceptance.
pub(super) fn removal(mut w: World) -> (result: (World, Outbox))
    requires domain(w), accepted(w), table(w),
    ensures accepted(result.0), result.1.len == 0,
        result.0.dependencies@.len() == if w.nodes@[0].phase == Phase::Ingesting { 1nat } else { 2nat },
        result.0.dependencies@[result.0.dependencies@.len() - 1] == w.dependencies@[1],
{
    let ghost before = w;
    proof { port_deps_apply::scope(w); reveal(domain); }
    let checked = w.check(); assert(checked.is_ok());
    let mut out = Outbox::new(); proof { reveal(empty_outbox); }
    apply(&mut w, Event::Fact { node: NodeId(0), fact: Fact::DependencyRemove {
        kind: PrerequisiteKind::Issue, number: 2761 } }, &mut out);
    proof { preserves(before, w); port_deps_apply::scope(w); }
    let checked = w.check(); assert(checked.is_ok());
    (w, out)
}

/// #806 original removal covers: retain Abandoned or Delivered while removing only in re-ingest.
pub(super) fn removal_cover(remove: bool, abandoned: bool) -> (result: (World, Outbox))
    ensures accepted(result.0), result.1.len == 0,
        result.0.dependencies@.len() == if remove { 1nat } else { 2nat },
        result.0.dependencies@[result.0.dependencies@.len() - 1].status
            == if abandoned { DependencyStatus::Abandoned } else { DependencyStatus::Delivered },
{
    let mut w = port_deps_witness::waiting_world();
    if remove { w.nodes[0].phase = Phase::Ingesting; }
    w.dependencies.push(Dependency { node: NodeId(0), target: DependencyTarget {
        kind: PrerequisiteKind::Issue, number: 2762, request: None },
        status: if abandoned { DependencyStatus::Abandoned } else { DependencyStatus::Delivered }, rejected: false });
    proof {
        reveal(port_deps_witness::blocked); reveal(port_deps_witness::original_node);
        reveal(initial_node); reveal(domain); reveal(accepted); reveal(dependencies_ok);
        reveal(table); reveal(retirement::node_rules); reveal(dependencies::declarations_unique);
        reveal(dependencies::zero_credit); dependencies::prerequisite_definition(w, NodeId(0));
        dependencies::delivered_definition(w.dependencies@, NodeId(0)); footprint::inline_is_valid(w, 0);
        assert forall|k: int| 0 <= k < w.dependencies@.len() implies w.dependencies@[k].node == NodeId(0)
            && w.dependencies@[k].target.request.is_none() && !w.dependencies@[k].rejected by { assert(k == 0 || k == 1); }
    }
    removal(w)
}
}
