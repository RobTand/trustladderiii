//! #806: failed-run branch algebra, independent of mutable world bookkeeping.
use super::*;

verus! {
/// Failed-run row: the first failure restarts the same operation once.
pub(super) proof fn restarted(before: Node, after: Node)
    requires before.run_failures == 0,
        after == (Node {
            pos: if before.phase == Phase::Making { 1u8 } else { 0u8 },
            generation: (before.generation + 1) as u64,
            run_failures: 1, ..port_review_retry::abandoned(before)
        }),
    ensures after == port_review_retry::failed_run(before),
{
    reveal(port_review_retry::failed_run);
}

/// Failed-run row: fold the saturated incident before mutating the world.
pub(super) proof fn incident(before: Node, ready: Node, needs_rob: bool)
    requires before.run_failures == 1, before.run_incidents <= 2,
        ready == (Node {
            pos: if before.phase == Phase::Making { 1u8 } else { 0u8 },
            generation: (before.generation + 1) as u64,
            run_failures: 0,
            run_incidents: if before.run_incidents < 2 { (before.run_incidents + 1) as u8 } else { 2u8 },
            ..port_review_retry::abandoned(before)
        }),
        needs_rob == (ready.run_incidents >= 2),
    ensures retirement::incident(ready, needs_rob) == port_review_retry::failed_run(before),
{
    reveal(port_review_retry::failed_run);
}
}
