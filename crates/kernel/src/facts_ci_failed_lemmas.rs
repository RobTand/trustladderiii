//! #806: isolate CI-failure retry frames and pre-enqueue input equivalence.
use super::*;

verus! {
/// #806 / Q54: verified paid-retry bounds imply the fact frame for an open node.
pub(super) proof fn retry_frame(before: World, after: World, i: usize,
    out_before: Outbox, out_after: Outbox)
    requires i < before.nodes@.len(), !(before.nodes@[i as int].phase is Closed),
        retry_bounds(before, after, i as int, out_before.len, out_after.len),
    ensures retirement::fact_frame(before, after, i, out_before, out_after),
{
    reveal(retry_bounds);
    reveal(retirement::fact_frame);
}

/// #806 / Q50: pre-enqueue cancellation does not alter the infrastructure result.
pub(super) proof fn cancellation_input(before: Node, prepared: Node)
    requires before.phase == Phase::Queued || (before.phase == Phase::Publishing
        && current_item(before) == Some(Item::Do(EffectKind::Enqueue))),
        prepared == (Node { effect: match before.effect {
            Some(e) => Some(InFlight { cancelling: e.cancelling
                || before.phase == Phase::Publishing, ..e }),
            None => None,
        }, ..before }),
    ensures retirement::fact_node(prepared, Fact::CiFailed { infra: true })
        == retirement::fact_node(before, Fact::CiFailed { infra: true }),
{
    reveal(retirement::fact_node);
}
}
