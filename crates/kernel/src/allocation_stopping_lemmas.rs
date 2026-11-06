//! #806: separate stopping rank and frame proofs from executable allocation.
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

/// #806 stopping row: exact stopped state implies every allocation frame clause.
pub(super) proof fn stopping_frame(before: Node, after: Node, outcome: Outcome)
    requires after == retirement::stopped(before, outcome),
        !(before.phase is Closed || before.phase is Stopping) ==> before.generation < u64::MAX,
    ensures
        after.repo == before.repo, after.footprint == before.footprint,
        (settle_shape(before) ==> settle_shape(after)),
        after.generation as int == before.generation as int
            + if before.phase is Closed || before.phase is Stopping { 0int } else { 1int },
        after.phase == match before.phase {
            Phase::Closed(o) => Phase::Closed(o),
            Phase::Stopping(o) => Phase::Stopping(if outcome == Outcome::Merged { outcome } else { o }),
            _ => Phase::Stopping(outcome),
        },
        after.next_seq == before.next_seq, after.credit == before.credit,
        after.last_cmd == before.last_cmd,
        after.review_stop == before.review_stop,
        (before.phase is Closed || before.phase is Stopping) ==> after == (Node {
            phase: after.phase, ..before
        }),
        !(before.phase is Closed || before.phase is Stopping) ==>
            after.pos == 0 && after.ticket.is_none() && after.question.is_none()
                && !after.waiting_incident && !after.incident_needs_rob,
{
    reveal(retirement::stopped);
}
}
