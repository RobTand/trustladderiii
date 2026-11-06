//! Q54 / #440 / #806: compare flag values once for every shared retry shape.
use super::*;

verus! {
/// Q54: share the real retry callees' exact contracts across diversion events.
/// kind 0 is ladder, 1 exhaustion, 2 planner retry, and 3 failed round.
pub(super) open spec fn retry(n: Node, fast: bool, id: u64, kind: u8, step: ReviewStop) -> (Node, u64) {
    if kind == 0 { port_review_retry::rung(n, fast, id, step) }
    else if kind == 1 { port_review_retry::room(n, fast, id, step) }
    else if kind == 2 { (port_failfast_diversion::plan_node(n, fast, true, id),
        if fast || n.plan_retries == 0 { id } else { (id + 1) as u64 }) }
    else { port_review_retry::round(n, fast, id, step) }
}

/// Q54: the original diversion check's definition of a work retry.
pub(super) open spec fn retried(before: Node, after: Node) -> bool {
    after.attempt > before.attempt || after.phase is Planning
        || match after.question { Some(q) => matches!(q.kind, QKind::CantMeet | QKind::OutOfCredit), None => false }
}

/// Q54 / #440: one parametric comparison covers all shared work-retry diversions.
/// A free fix round is identical for both flag values and therefore does not divert.
pub(super) proof fn diversion(n: Node, id: u64, kind: u8, step: ReviewStop)
    requires kind < 4, n.generation < u64::MAX, id < u64::MAX,
        !(n.phase is Stopping || n.phase is Closed),
        kind == 2 ==> n.phase is Planning && step == ReviewStop::PlanCantMeet,
    ensures {
        let off = retry(n, false, id, kind, step);
        let on = retry(n, true, id, kind, step);
        on != off ==> on.0.phase == Phase::Stopping(Outcome::Stopped)
            && on.0.review_stop == Some(step) && on.0.credit == n.credit
            && on.0.question.is_none() && on.1 == id
            && on.0.attempt == n.attempt && on.0.replanned == n.replanned
            && retried(n, off.0)
    },
{
    reveal(port_failfast_diversion::plan_node);
    reveal(retirement::stopped);
}
}
