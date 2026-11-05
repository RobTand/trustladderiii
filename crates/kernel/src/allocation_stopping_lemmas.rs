//! #806: isolate stopping progress arithmetic from executable allocation frames.
use super::*;

verus! {
/// #805 / #806 stopping row: an unobstructed open phase precedes teardown.
pub(super) proof fn active_distance_lower(n: Node)
    requires !n.paused, !n.waiting_incident, n.effect.is_none(),
        !(n.phase is Closed || n.phase is Stopping),
    ensures 192 <= distance(n),
{
    reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank);
}

/// #805 / #806 stopping row: the first teardown position has fixed distance.
pub(super) proof fn stopping_distance(n: Node)
    requires n.phase is Stopping, n.pos == 0, n.question.is_none(),
        !n.waiting_incident, n.effect.is_none(),
    ensures distance(n) == 138,
{
    reveal(distance);
}

/// #805 / #806 stopping row: compose exact handler output with rank bounds.
pub(super) proof fn stopping_progress(before: Node, after: Node, outcome: Outcome)
    requires after == retirement::stopped(before, outcome),
    ensures !before.paused && !before.waiting_incident && before.effect.is_none()
        && !(before.phase is Closed || before.phase is Stopping)
        ==> distance(after) < distance(before),
{
    if !before.paused && !before.waiting_incident && before.effect.is_none()
        && !(before.phase is Closed || before.phase is Stopping) {
        reveal(retirement::stopped);
        active_distance_lower(before);
        stopping_distance(after);
    }
}
}
