//! #771/#805: unchanged world predicate lemmas, independently gated.
use super::*;

verus! {
/// #805: the production checker projection, exposed to its proof modules.
pub(super) proof fn world_checked_definition(w: World)
    ensures world_checked(w) == (w.gates@.len() > 0 && w.pool_capacity > 0
        && (forall|j: int| 0 <= j < w.gates@.len() ==> w.gates@[j].w > 0)
        && (forall|j: int| 0 <= j < w.nodes@.len() ==> node_checked(w, w.nodes@[j]))
        && dependencies::dependencies_checked(w)),
{
    reveal(world_checked);
}


/// #805: the production checker projection, exposed to its proof modules.
pub(super) proof fn settled_world_definition(w: World)
    ensures settled_world(w) == (forall|i: int| 0 <= i < w.nodes@.len() ==> !movable(w, i)),
{
    reveal(settled_world);
}


/// #771: the production checker and enforced reserves supply the step scope.
pub(super) proof fn checked_reserves(w: World)
    requires world_checked(w), world_headroom(w), footprint::ordered_table(w),
    ensures step_scope(w), settle_ready(w),
{
    reveal(world_checked);
    reveal(world_headroom);
    reveal(step_scope);
    settle_reserve_totals(w.nodes@);
    assert forall|j: int| 0 <= j < w.nodes@.len() implies
        w.nodes@[j].generation as int + generation_capacity(w.nodes@[j]) <= u64::MAX as int
            && w.nodes@[j].next_seq as int + effect_capacity(w.nodes@[j]) <= u64::MAX as int
            && w.nodes@[j].credit.earned as int + merge_capacity(w.nodes@[j]) <= u64::MAX as int
    by { local_capacities(w.nodes@[j]); }
    open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
    reveal(open_prefix);
}


/// Q30/Q39 / #771: scheduling depends on these storage views, not Vec identity.
pub(super) proof fn movable_frame(before: World, after: World, i: int)
    requires before.nodes@ == after.nodes@, before.gates@ == after.gates@,
        before.pool_capacity == after.pool_capacity,
        before.dependencies@ == after.dependencies@,
    ensures movable(before, i) == movable(after, i),
{
    assert(oldest_waiter(before, i) == oldest_waiter(after, i));
}

}
