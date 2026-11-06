//! #806 W7: the three original dependency apply harnesses, without counter bounds.
use super::port_deps_single::{accepted, dependencies_ok, domain};
use super::port_deps_specs::*;
use super::*;

verus! {
/// #806: real consistency supplies all dependency-event caller premises, without reserves.
pub(super) proof fn scope(w: World)
    requires domain(w), accepted(w),
    ensures port_deps_single_domain(w), port_deps_single_accepted(w),
        footprint::ordered_table(w), storage_ordered(w), dependencies::edges_indexable(w),
        forall|j: int| 0 <= j < w.nodes@.len() ==> arithmetic_consistent(w.nodes@[j])
            && settle_shape(w.nodes@[j]) && stopping_effect_fits(w.nodes@[j])
            && (w.nodes@[j].phase is Closed ==> w.nodes@[j].effect.is_none())
            && footprint::valid_footprint(w, w.nodes@[j].footprint),
{
    reveal(accepted); reveal(retirement::node_rules); reveal(domain); reveal(dependencies_ok);
    reveal(port_deps_single_domain); reveal(port_deps_single_accepted);
    reveal(footprint::ordered_table); reveal(storage_ordered); dependencies::indexable_definition(w);
}

/// #806: an exact external declaration preserves the original arbitrary node's complete checks.
pub(super) proof fn external_preserves(w: World, v: World, target: DependencyTarget,
    status: DependencyStatus, out: Outbox, next: Outbox)
    requires domain(w), accepted(w), w.dependencies@.len() <= 1,
        target.kind == PrerequisiteKind::Issue, target.number > 0, target.request.is_none(),
        w.dependencies@.len() == 1 ==> w.dependencies@[0].target == target,
        port_deps_apply_specs::external_post(w, v, 0, target, status, out, next),
    ensures domain(v), accepted(v),
{
    reveal(accepted); reveal(domain); reveal(dependencies_ok); reveal(dependencies::declarations_unique);
    reveal(port_deps_apply_specs::external_post); reveal(port_deps_apply_specs::prepared);
    reveal(key); reveal(unchanged); reveal(dependencies::dependency_frame);
    reveal(retirement::node_rules); reveal(dependencies::zero_credit);
    dependencies::prerequisite_definition(w, NodeId(0)); dependencies::prerequisite_definition(v, NodeId(0));
    dependencies::delivered_definition(w.dependencies@, NodeId(0));
    dependencies::delivered_definition(v.dependencies@, NodeId(0));
    footprint::storage_frame(w, v, w.nodes@[0].footprint, w.nodes@[0].footprint);
    if w.dependencies@.len() == 1 {
        assert(exists|k: usize| k < w.dependencies@.len() && key(w.dependencies@[k as int], 0, target)) by {
            assert(key(w.dependencies@[0], 0, target));
        }
    }
    assert(v.dependencies@.len() <= 1);
    assert forall|k: int| 0 <= k < v.dependencies@.len() implies v.dependencies@[k].node == NodeId(0)
        && v.dependencies@[k].target.request.is_none() && !v.dependencies@[k].rejected by { assert(k == 0); }
}

/// #806 apply_dependency: all statuses, exact terminal retention, phase and complete checker.
pub(super) fn dependency_fact(mut w: World, status: DependencyStatus) -> (result: (World, Outbox))
    requires domain(w), accepted(w), w.dependencies@.len() == 1,
        w.dependencies@[0].target == (DependencyTarget { kind: PrerequisiteKind::Issue, number: 2761, request: None }),
    ensures accepted(result.0), result.0.nodes@[0].phase == w.nodes@[0].phase,
        result.0.dependencies@.len() == 1,
        result.0.dependencies@[0].status == if w.dependencies@[0].status == DependencyStatus::Pending {
            status
        } else { w.dependencies@[0].status },
{
    let ghost before = w;
    proof { scope(w); reveal(domain); reveal(accepted); reveal(dependencies_ok); reveal(empty_outbox); }
    let checked = w.check(); assert(checked.is_ok());
    let target = DependencyTarget { kind: PrerequisiteKind::Issue, number: 2761, request: None };
    let mut out = Outbox::new(); let ghost before_out = out; proof { reveal(empty_outbox); }
    apply(&mut w, Event::Fact { node: NodeId(0), fact: Fact::Dependency { target, status } }, &mut out);
    proof {
        external_preserves(before, w, target, status, before_out, out);
        reveal(port_deps_apply_specs::external_post); reveal(port_deps_apply_specs::prepared);
        reveal(key); reveal(unchanged); reveal(dependencies::dependency_frame);
        scope(w);
    }
    let checked = w.check(); assert(checked.is_ok());
    (w, out)
}

/// #806 apply_dependency_registration: every original phase and full-width ledger is covered.
pub(super) fn registration(mut w: World, status: DependencyStatus) -> (result: (World, Outbox))
    requires domain(w), accepted(w), w.dependencies@.len() == 0,
    ensures accepted(result.0),
        result.0.nodes@[0].planning_started == w.nodes@[0].planning_started,
        result.0.nodes@[0].phase == w.nodes@[0].phase,
        result.0.dependencies@.len() == if w.nodes@[0].planning_started
            || !(w.nodes@[0].phase is Ingesting || w.nodes@[0].phase is Auditing || w.nodes@[0].phase is Unplanned) {
            0nat
        } else { 1nat },
{
    let ghost before = w;
    proof { scope(w); reveal(domain); reveal(accepted); reveal(dependencies_ok); }
    let checked = w.check(); assert(checked.is_ok());
    let target = DependencyTarget { kind: PrerequisiteKind::Issue, number: 2762, request: None };
    let mut out = Outbox::new(); let ghost before_out = out;
    proof { reveal(empty_outbox); }
    apply(&mut w, Event::Fact { node: NodeId(0), fact: Fact::Dependency { target, status } }, &mut out);
    proof {
        external_preserves(before, w, target, status, before_out, out);
        reveal(port_deps_apply_specs::external_post); reveal(port_deps_apply_specs::prepared);
        reveal(unchanged); reveal(dependencies::dependency_frame); scope(w);
    }
    let checked = w.check(); assert(checked.is_ok());
    (w, out)
}
}
