//! K5 frozen export E5.1 lift (CONTRACTS.md v2; owner Advance). A separate module, so consumers that
//! need internal_safety::step_premises call this lemma; no consumer reveals step_rows itself.
use super::*;

verus! {
/// One advance of node i as an internal_safety step: step_rows, advance's existing world frames and the
/// E5.3/E5.5 grant clauses give step_premises plus the other premises of step_preserves_accepted_full.
pub(super) proof fn step_premises_lift(before: World, after: World, i: int)
    requires checking::accepted(before), scheduling_safe(before),
        before.gates@.len() <= u64::MAX, before.nodes@.len() <= u64::MAX, 0 <= i < before.nodes@.len(),
        advance_family_post::step_rows(before, after, i),
        after.nodes@ == before.nodes@.update(i, after.nodes@[i]),
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@, after.pool_capacity == before.pool_capacity,
        after.next_question == before.next_question, before.next_ticket <= after.next_ticket,
        !before.nodes@[i].holds && after.nodes@[i].holds ==> scheduler::slot_spec(before, i),
        !before.nodes@[i].pool && after.nodes@[i].pool
            ==> pool_users(before.nodes@, before.nodes@.len()) < before.pool_capacity,
    ensures internal_safety::step_premises(before, after, i),
        before.nodes@[i].phase is Closed ==> after.nodes@[i].phase is Closed,
        after.nodes@[i].credit.withheld ==> before.nodes@[i].credit.withheld
            || !dependencies::prerequisites_delivered(before, NodeId(i as u64)),
        dependencies::prerequisites_delivered(before, NodeId(i as u64))
            || dependencies::owner_waiting_ok(after.nodes@[i]),
{
    reveal(advance_family_post::step_rows);
}

}
