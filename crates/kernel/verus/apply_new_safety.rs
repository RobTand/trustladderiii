//! K5 New* C2 producers. Spec bodies match internal_safety's C2 interface.
use super::*;

verus! {
pub(super) proof fn adoption_c2(before: World, after: World, repo: RepoId)
    requires repo.0 < before.gates@.len(),
        after.nodes@.len() == before.nodes@.len() + 1,
        forall|j: int| 0 <= j < before.nodes@.len() ==> after.nodes@[j] == before.nodes@[j],
        after.gates@ == before.gates@, after.pool_capacity == before.pool_capacity,
        after.dependencies@ == before.dependencies@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket, after.next_question == before.next_question,
        initial_node(Node { priority: Priority::Unranked, proposed: None,
            ..after.nodes@[before.nodes@.len() as int] }, repo),
    ensures after.nodes@.len() == before.nodes@.len() + 1
        && (forall|j: int| 0 <= j < before.nodes@.len() ==> #[trigger] after.nodes@[j] == before.nodes@[j])
        && after.gates@ == before.gates@ && after.pool_capacity == before.pool_capacity
        && after.dependencies@ == before.dependencies@
        && (after.file_sets@.len() >= before.file_sets@.len()
            && forall|j: int| 0 <= j < before.file_sets@.len()
                ==> #[trigger] after.file_sets@[j] == before.file_sets@[j])
        && before.next_ticket <= after.next_ticket && before.next_question <= after.next_question,
        retirement::node_rules(after, after.nodes@[before.nodes@.len() as int])
            && !after.nodes@[before.nodes@.len() as int].holds
            && !after.nodes@[before.nodes@.len() as int].pool
            && after.nodes@[before.nodes@.len() as int].ticket.is_none()
            && !after.nodes@[before.nodes@.len() as int].credit.withheld,
{
    reveal(initial_node);
    handler_safety::initial_rules(after, after.nodes@[before.nodes@.len() as int], repo);
}
}
