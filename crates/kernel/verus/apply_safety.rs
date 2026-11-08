//! K4b #910: apply's consequences in the common transition-safety vocabulary.
use super::*;

verus! {

/// K4b: apply's existing world-frame postconditions are exactly the Apply
/// frame arm of transition_safety. Consumers call this after the real apply
/// instead of restating the frame clauses.
pub(super) proof fn apply_frame(before: World, after: World)
    requires before.nodes@.len() <= after.nodes@.len() <= before.nodes@.len() + 1,
        after.next_ticket == before.next_ticket,
        after.next_question as int <= before.next_question as int + 1,
        after.gates@ == before.gates@, after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast,
        after.file_sets@.len() >= before.file_sets@.len(),
        forall|j: int| 0 <= j < before.file_sets@.len()
            ==> after.file_sets@[j] == before.file_sets@[j],
        forall|j: int| 0 <= j < before.nodes@.len()
            ==> after.nodes@[j].repo == before.nodes@[j].repo,
    ensures transition_safety::apply_frame(before, after),
{
}

/// check_nodes for the selected fact rows: on_fact's exact node update keeps
/// every per-node rule, including the single late-merge credit, with no
/// checker premise.
pub(super) proof fn selected_fact_step(b: Node, fact: Fact)
    requires retirement::selected(fact),
    ensures transition_safety::node_step(b, retirement::fact_node(b, fact)),
{
    reveal(retirement::fact_node); reveal(retirement::stopped);
}

/// check_apply Conflict row from on_fact's exact node update: waiting and live
/// publication rebase with one generation and unchanged credit and slot;
/// a repeated Conflict changes nothing.
pub(super) proof fn conflict_rule(b: Node)
    requires b.generation < u64::MAX,
    ensures ({
        let n = retirement::fact_node(b, Fact::Conflict);
        if b.phase == Phase::AwaitingDelivery || b.phase == Phase::Publishing {
            n.phase == (if b.phase == Phase::AwaitingDelivery {
                Phase::Ready(Resume::Rebasing) } else { Phase::Ready(Resume::Rebasing) })
                && n.generation == b.generation + 1
                && n.credit == b.credit && n.holds == b.holds
        } else { n == b }
    }),
{
    reveal(retirement::fact_node);
}

}
