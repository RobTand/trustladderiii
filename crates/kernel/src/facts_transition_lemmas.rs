//! #806: expose verified fact-handler frames without unfolding them in every arm.
use super::*;

verus! {
/// #806 fact rows: the existing handler frame implies the original caller clauses.
pub(super) proof fn frame_consequences(before: World, after: World, i: usize,
    out_before: Outbox, out_after: Outbox)
    requires i < before.nodes@.len(),
        retirement::fact_frame(before, after, i, out_before, out_after),
    ensures
        (before.nodes@[i as int].phase is Closed ==> after.nodes@[i as int].phase is Closed),
        after.nodes@[i as int].repo == before.nodes@[i as int].repo,
        after.nodes@[i as int].footprint == before.nodes@[i as int].footprint,
        (settle_shape(before.nodes@[i as int]) ==> settle_shape(after.nodes@[i as int])),
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> after.nodes@[j] == before.nodes@[j],
        after.nodes@[i as int].generation as int <= before.nodes@[i as int].generation as int + 1,
        after.nodes@[i as int].next_seq == before.nodes@[i as int].next_seq,
        after.nodes@[i as int].credit.start == before.nodes@[i as int].credit.start,
        after.nodes@[i as int].credit.earned == before.nodes@[i as int].credit.earned,
        after.nodes@[i as int].credit.added == before.nodes@[i as int].credit.added,
        after.nodes@[i as int].credit.spent as int <= before.nodes@[i as int].credit.spent as int + 1,
        after.next_ticket == before.next_ticket,
        after.next_question as int <= before.next_question as int + 1,
        after.gates@ == before.gates@,
        after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast,
        after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@,
        out_after.len <= out_before.len + 2,
        after.nodes@[i as int].last_cmd == before.nodes@[i as int].last_cmd,
{
    reveal(retirement::fact_frame);
}
}
