//! Failed-run branch facts, separate from mutable retry bookkeeping.
use super::*;

verus! {
/// Q30: abandoning an operation releases ownership and cancels its pending start.
pub(super) open spec fn abandoned(n: Node) -> Node {
    Node { run: None, job: None, pool: false,
        effect: match n.effect { Some(e) => Some(InFlight { cancelling: e.cancelling
            || matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }), None => None }, ..n }
}

/// Q50 failed-run row: the first failure restarts the same operation once.
pub(super) proof fn restarted(before: Node, after: Node)
    requires before.run_failures == 0,
        after == (Node {
            pos: if before.phase == Phase::Making { 1u8 } else { 0u8 },
            generation: (before.generation + 1) as u64,
            run_failures: 1, ..abandoned(before)
        }),
    ensures after == port_t860_contracts::failed_node(before),
{
    reveal(port_t860_contracts::failed_node);
}

/// Q50 failed-run row: fold the saturated incident before mutating the world.
pub(super) proof fn incident(before: Node, ready: Node, needs_rob: bool)
    requires before.run_failures == 1, before.run_incidents <= 2,
        ready == (Node {
            pos: if before.phase == Phase::Making { 1u8 } else { 0u8 },
            generation: (before.generation + 1) as u64,
            run_failures: 0,
            run_incidents: if before.run_incidents < 2 { (before.run_incidents + 1) as u8 } else { 2u8 },
            ..abandoned(before)
        }),
        needs_rob == (ready.run_incidents >= 2),
    ensures retirement::incident(ready, needs_rob) == port_t860_contracts::failed_node(before),
{
    reveal(port_t860_contracts::failed_node);
}

/// Q50: package the exact result and notice frame without exposing either to callers.
pub(super) proof fn failed_result(before: Node, after: Node, bo: Outbox, ao: Outbox)
    requires after == port_t860_contracts::failed_node(before),
        retirement::notices_only(bo) ==> retirement::notices_only(ao),
    ensures port_t860_contracts::failed_post(before, after, bo, ao),
{
    reveal(port_t860_contracts::failed_post);
}
}
