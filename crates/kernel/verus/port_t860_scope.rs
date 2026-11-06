//! #860: reuse the approved original-domain scope and checker headroom.
use super::*;

verus! {
/// #860: derive the real apply and complete-checker premises from the same or
/// weaker original-domain construction; no extra invariant is assumed by callers.
pub(super) proof fn apply_scope(w: World)
    requires retirement::domain(w), retirement::accepted(w), retirement::bounded(w), storage_ordered(w),
    ensures world_headroom(w), footprint::ordered_table(w), dependencies::edges_indexable(w),
        forall|i: int| 0 <= i < w.nodes@.len() ==> node_headroom(w.nodes@[i])
            && arithmetic_consistent(w.nodes@[i]) && settle_shape(w.nodes@[i])
            && stopping_effect_fits(w.nodes@[i])
            && (w.nodes@[i].phase is Closed ==> w.nodes@[i].effect.is_none())
            && footprint::valid_footprint(w, w.nodes@[i].footprint),
{
    reveal(retirement::accepted); reveal(retirement::node_rules); reveal(retirement::bounded);
    reveal(storage_ordered); reveal(world_headroom);
    open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
    reveal_with_fuel(open_prefix, 3);
    assert forall|i: int| 0 <= i < w.nodes@.len() implies node_headroom(w.nodes@[i]) by {}
    dependencies::indexable_definition(w);
}

/// #860: original two-node counter bounds leave headroom for the real complete
/// checker after one apply, including the largest ingest deposit.
pub(super) proof fn checker_headroom(before: World, after: World)
    requires retirement::domain(before), retirement::bounded(before), after.nodes@.len() == 2,
        after.next_ticket == before.next_ticket,
        after.next_question as int <= before.next_question as int + 1,
        forall|i: int| 0 <= i < 2 ==> {
            let b = before.nodes@[i]; let n = #[trigger] after.nodes@[i];
            n.generation as int <= b.generation as int + 2
                && n.next_seq as int <= b.next_seq as int + 1
                && n.credit.start as int <= b.credit.start as int + CREDIT_START_STEP_BUDGET
                && n.credit.earned as int <= b.credit.earned as int + 1
                && n.credit.added as int <= b.credit.added as int + CREDIT_ADDED_STEP_BUDGET
                && n.credit.spent as int <= b.credit.spent as int + CREDIT_SPENT_STEP_BUDGET
        },
    ensures world_headroom(after),
{
    reveal(retirement::bounded); reveal(world_headroom);
    open_prefix_monotone(after.nodes@, 0, 2);
    reveal(open_prefix);
    assert forall|i: int| 0 <= i < 2 implies node_headroom(after.nodes@[i]) by {
        assert(node_headroom(before.nodes@[i]));
    }
}

}
