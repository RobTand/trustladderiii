//! Q54 / #440 / #806: checker composition for the maker-files credit exception.
use super::*;

verus! {
/// Q54 question allocation: increasing next_question preserves an unchanged
/// node's complete rules. This extends retirement's equality frame locally.
pub(super) proof fn question_frame(before: World, after: World, n: Node)
    requires before.gates@ == after.gates@, before.file_sets@ == after.file_sets@,
        before.next_ticket == after.next_ticket, before.next_question <= after.next_question,
        retirement::node_rules(before, n),
    ensures retirement::node_rules(after, n),
{
    reveal(retirement::node_rules);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}

/// Q54 maker-files funding row: real operation completion followed by the
/// OutOfCredit/Ready(Making) question satisfies every production node rule.
pub(super) proof fn extension_node(before: World, after: World, n: Node, joined: Footprint)
    requires retirement::node_rules(before, n), n.phase == Phase::Making,
        n.generation < u64::MAX, before.next_question < u64::MAX,
        before.gates@ == after.gates@, before.file_sets@ == after.file_sets@,
        before.next_ticket == after.next_ticket,
        after.next_question as int == before.next_question as int + 1,
        footprint::valid_footprint(after, joined), !footprint::footprint_empty(joined),
    ensures retirement::node_rules(after, port_review_retry::asked(
        Node { footprint: joined, ..port_review_reply::ended(n) }, before.next_question,
        QKind::OutOfCredit, Cont::Ready(Resume::Making))),
{
    reveal(retirement::node_rules);
}

/// Q54 maker-files funding exception: compose the local node result and the
/// monotone question frame with retirement's complete two-node checker predicate.
pub(super) proof fn extension_world(before: World, after: World, joined: Footprint)
    requires retirement::domain(before), retirement::accepted(before),
        node_headroom(before.nodes@[0]), before.next_question < u64::MAX,
        before.nodes@[0].phase == Phase::Making,
        footprint::valid_footprint(after, joined), !footprint::footprint_empty(joined),
        after.nodes@.len() == 2, after.nodes@[1] == before.nodes@[1],
        after.nodes@[0] == port_review_retry::asked(
            Node { footprint: joined, ..port_review_reply::ended(before.nodes@[0]) },
            before.next_question, QKind::OutOfCredit, Cont::Ready(Resume::Making)),
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@, after.pool_capacity == before.pool_capacity,
        after.next_ticket == before.next_ticket,
        after.next_question as int == before.next_question as int + 1,
    ensures retirement::domain(after), retirement::accepted(after),
        after.nodes@[0].phase == Phase::AwaitingRob,
        after.nodes@[0].credit == before.nodes@[0].credit,
        after.nodes@[0].review_stop.is_none(),
        after.nodes@[0].attempt == before.nodes@[0].attempt,
        after.nodes@[0].replanned == before.nodes@[0].replanned,
        after.nodes@[0].proposed == before.nodes@[0].proposed,
        after.nodes@[0].adopted == before.nodes@[0].adopted,
        after.nodes@[0].next_seq == before.nodes@[0].next_seq,
        after.nodes@[0].question == Some(Question { id: before.next_question,
            kind: QKind::OutOfCredit, cont: Cont::Ready(Resume::Making), cost: 1, clarified: false }),
{
    reveal(retirement::accepted);
    extension_node(before, after, before.nodes@[0], joined);
    question_frame(before, after, before.nodes@[1]);
    reveal(retirement::node_rules);
}
}
