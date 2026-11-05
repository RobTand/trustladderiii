//! #771/#805: rank depends on agenda state, not resource-cleanup fields.
use super::*;

verus! {
/// Confirmed-effect rows: resource cleanup leaves every rank input unchanged.
pub(super) proof fn distance_frame(before: Node, after: Node)
    requires after.phase == before.phase, after.pos == before.pos,
        after.paused == before.paused, after.waiting_incident == before.waiting_incident,
        after.effect == before.effect, after.question == before.question,
    ensures distance(after) == distance(before),
{
    reveal(distance);
}

/// Cleanup question row: consuming a real Stopping agenda entry lowers distance
/// by two; run, pool, job, PR and worktree cleanup cannot affect that arithmetic.
pub(super) proof fn teardown_distance(before: Node, after: Node)
    requires after.phase == before.phase, after.pos as int == before.pos as int + 1,
        after.paused == before.paused, after.waiting_incident == before.waiting_incident,
        after.effect == before.effect, after.question == before.question,
        current_item(before).is_some(),
    ensures (before.effect.is_none() && before.question.is_none() && !before.waiting_incident
        && before.phase is Stopping ==> distance(after) + 2 <= distance(before)),
{
    reveal(distance);
}
}
