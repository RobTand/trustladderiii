//! #806 phase progress: bridge the original bounded construction to #805.
use super::*;
verus! {
/// #806: no local internal move remains, even before scheduler obstruction.
pub closed spec fn zero_internal_height(w: World) -> bool {
    potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)) == 0
}

/// #806: expose the closed settledness premise to the real checker and cover.
pub(super) proof fn zero_internal_height_definition(w: World)
    ensures zero_internal_height(w)
        == (potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)) == 0),
{
    reveal(zero_internal_height);
}

/// #806: weaker than the original constructor: no head, width, pool, or
/// auxiliary counter bounds, and credit may reach the original counter limit.
pub(super) open spec fn domain(w: World) -> bool {
    retirement::domain(w) && retirement::bounded(w) && w.file_sets@.len() == 0
        && forall|j: int| 0 <= j < w.nodes@.len() ==> w.nodes@[j].last_cmd < 1099511627776u64
}

/// Q30/Q39 / #806: the original bounds imply #771 reserves and ordered storage.
pub(super) proof fn reserves(w: World)
    requires domain(w),
    ensures world_headroom(w), fresh_commands(w), storage_ordered(w),
{
    reveal(retirement::bounded); reveal(world_headroom); reveal(fresh_commands);
    reveal(storage_ordered); reveal(footprint::ordered_table);
    open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
    reveal(open_prefix);
    assert forall|j: int| 0 <= j < w.nodes@.len() implies node_headroom(w.nodes@[j]) by {}
}
}

kernel_fn! {
/// Q30/Q39 / #806: run the real complete checker before invoking #805.
/// Its existing two-node acceptance equivalence retains all original checks.
pub(super) fn scope(w: &World) -> (result: ()) [
    requires domain(*w), retirement::accepted(*w), settled_world(*w),
        exists|j: int| 0 <= j < w.nodes@.len() && !(w.nodes@[j].phase is Closed),
    ensures progress_scope(*w),
] proof [reserves(*w); reveal(retirement_domain); reveal(retirement_accepted);] {
    let checked = w.check();
    assert!(checked.is_ok());
    kernel_proof! { reveal(progress_scope); }
}
}

kernel_fn! {
/// #806: two real node-distance reads prove the original sum cannot overflow.
/// Each verified distance is at most 4096; dependencies are empty.
pub(super) fn bound(w: &World) -> (result: ()) [
    requires w.nodes@.len() == 2, w.dependencies@.len() == 0,
    ensures progress_measure(*w) <= 8192,
] {
    let _first = w.nodes[0].distance();
    let _second = w.nodes[1].distance();
    kernel_proof! {
        ranks::node_distance_definition(w.nodes@[0]);
        ranks::node_distance_definition(w.nodes@[1]);
        progress::measure_definition(*w, 0);
        reveal_with_fuel(node_total, 3);
        assert(w.nodes@.drop_last().last() == w.nodes@[0]);
        assert(w.nodes@.last() == w.nodes@[1]);
    }
}
}

kernel_fn! {
/// Q30/Q39 / #806: selected input exists and real apply alone lowers measure.
/// Universal over the weaker original construction, independent of phase.
pub(super) fn verify_apply(mut w: World) -> (result: (World, Event, u64, u64)) [
    requires domain(w), retirement::accepted(w), settled_world(w),
        exists|j: int| 0 <= j < w.nodes@.len() && !(w.nodes@[j].phase is Closed),
    ensures witness_selection(w, Some(result.1)), progress_input(w, result.1),
        result.2 as nat == progress_measure(w),
        result.3 as nat == progress_measure(result.0), result.3 < result.2,
        progress_measure(result.0) < progress_measure(w),
] {
    scope(&w);
    bound(&w);
    kernel_proof! { dependencies::indexable_definition(w); }
    kernel_snapshot! { before = w }
    let before_measure = w.measure();
    kernel_proof! {
        reveal(progress_scope); reveal(storage_ordered);
        outside_branch_exists(w); checked_reserves(w);
        reveal(step_scope);
    }
    let selected = w.witness();
    kernel_proof! { reveal(witness_selection); }
    assert!(selected.is_some());
    let event = selected.unwrap();
    kernel_proof! { reveal(progress_input); reveal(witness_event); }
    let mut out = Outbox::new();
    kernel_proof! { reveal(empty_outbox); }
    apply(&mut w, event, &mut out);
    let after_measure = w.measure();
    (w, event, before_measure, after_measure)
}
}
