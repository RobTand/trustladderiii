//! #771: isolate cleanup metric expansion from the real command and Ack bodies.
use super::*;

verus! {
/// Cleanup question row / #771: consuming a Stopping teardown lowers rank by two.
pub(super) proof fn teardown_progress(before: Node, after: Node)
    requires before.phase is Stopping, before.effect.is_none(), before.question.is_none(),
        !before.waiting_incident, matches!(current_item(before), Some(Item::Teardown(_))),
        after.phase == before.phase, after.pos as int == before.pos as int + 1,
        after.effect.is_none(), after.question.is_none(), !after.waiting_incident,
    ensures distance(after) + 2 == distance(before),
{
    reveal(distance);
    assert(before.pos < 5);
}

/// Confirmed-effect rows / #771: resource and failure-counter cleanup preserves rank.
pub(super) proof fn resource_distance(before: Node, after: Node)
    requires after.phase == before.phase, after.pos == before.pos,
        after.effect == before.effect, after.question == before.question,
        after.paused == before.paused, after.waiting_incident == before.waiting_incident,
    ensures distance(after) == distance(before),
{
    reveal(distance);
}
}
