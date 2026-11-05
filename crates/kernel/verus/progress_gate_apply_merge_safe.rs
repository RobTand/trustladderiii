//! State-table gate apply / #806: complete consistency after the real update.
use super::progress_gate_apply::*;
use super::*;

verus! {
/// #806: merge retains complete consistency via the existing verified fact rules.
/// Four original nodes bound only this safety composition, not #805's rank lemma.
pub(super) proof fn merge_safe(before: World, after: World, head: HeadId)
    requires merge_input(before), node_headroom(before.nodes@[0]),
        after.nodes@ == before.nodes@.update(0,
            retirement::fact_node(before.nodes@[0], Fact::PrMerged { head, checks_passed: true })),
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket, after.next_question == before.next_question,
        after.pool_capacity == before.pool_capacity, after.dependencies@ == before.dependencies@,
    ensures domain(after), accepted(after),
{
    reveal(accepted); reveal(retirement::node_rules); reveal(retirement::fact_node);
    reveal_with_fuel(potential_prefix, 5);
    retirement::fact_preserves_node(before, before.nodes@[0], Fact::PrMerged { head, checks_passed: true });
    assert forall|j: int| 0 <= j < 4 implies retirement::node_rules(after, after.nodes@[j]) by {
        retirement::node_frame(before, after, after.nodes@[j]);
    }
    assert forall|a: int, b: int| 0 <= a < b < 4 implies pair(after, a, b) by {
        assert(pair(before, a, b));
        footprint::storage_frame(before, after, before.nodes@[a].footprint, before.nodes@[b].footprint);
    }
    assert forall|r: int| 0 <= r < after.gates@.len() implies
        holders(after, RepoId(r as u64), 4) <= after.gates@[r].w by {
        assert(holders(after, RepoId(r as u64), 4) <= holders(before, RepoId(r as u64), 4));
    }
}

}
