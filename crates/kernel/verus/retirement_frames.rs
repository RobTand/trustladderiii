//! #806: independently gated safety obligations over the unchanged real kernel.
use super::retirement::*;
use super::*;

verus! {
/// State-table consistency is insensitive to a world's Vec allocation identity.
pub(super) proof fn node_frame(before: World, after: World, n: Node)
    requires before.gates@ == after.gates@, before.file_sets@ == after.file_sets@,
        before.next_ticket == after.next_ticket, before.next_question == after.next_question,
    ensures node_rules(before, n) == node_rules(after, n),
{
    reveal(node_rules);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}
}

verus! {
/// State-table infrastructure row / #806: derive the common frame separately
/// from the executable branch proof to bound each solver query.
pub(super) proof fn infrastructure_frame(before: World, after: World, i: usize,
    out_before: Outbox, out_after: Outbox)
    requires i < before.nodes@.len(),
        node_headroom(before.nodes@[i as int]),
        after.nodes@[i as int] == fact_node(before.nodes@[i as int], Fact::CiFailed { infra: true }),
        before.nodes@[i as int].phase == Phase::Queued ||
            (before.nodes@[i as int].phase == Phase::Publishing
                && current_item(before.nodes@[i as int]) == Some(Item::Do(EffectKind::Enqueue))),
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != i ==> after.nodes@[j] == before.nodes@[j],
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@, after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast, after.next_ticket == before.next_ticket,
        after.next_question == before.next_question, out_after.len <= out_before.len + 1,
    ensures fact_frame(before, after, i, out_before, out_after),
{
    reveal(fact_node); reveal(fact_frame);
}
}

verus! {
/// State-table Q50 / #806: compose the actual counter helper and incident helper.
pub(super) proof fn infrastructure_result(before: Node, after: Node, escalation: Option<bool>)
    requires escalation == infrastructure_retry(before).1,
        after == match escalation {
            Some(needs_rob) => incident(infrastructure_retry(before).0, needs_rob),
            None => infrastructure_retry(before).0,
        },
        before.phase == Phase::Queued || (before.phase == Phase::Publishing
            && current_item(before) == Some(Item::Do(EffectKind::Enqueue))),
    ensures after == fact_node(before, Fact::CiFailed { infra: true }),
{
    reveal(infrastructure_retry); reveal(incident); reveal(fact_node);
}
}
