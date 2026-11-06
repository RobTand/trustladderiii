//! Q54 / #440 / #806: compare flag values once for every shared retry shape.
use super::*;

verus! {
/// Q54: consume the real retry contracts. The ladder and planner ordinary
/// branches need only the retry outcome asserted by the original paired check.
pub(super) open spec fn retry_post(n: Node, after: (Node, u64), fast: bool,
    id: u64, kind: u8, step: ReviewStop) -> bool {
    if kind == 2 {
        port_failfast_diversion::plan_fields(n, after.0, fast, true, id)
            && after.1 == if fast || n.plan_retries == 0 { id } else { (id + 1) as u64 }
    } else if kind == 0 {
        if fast { after == (port_review_retry::review_stopped(n, step), id) }
        else { retried(n, after.0) }
    } else if kind == 1 { after == port_review_retry::room(n, fast, id, step) }
    else { after == port_review_retry::round(n, fast, id, step) }
}

/// Q54: the original diversion check's definition of a work retry.
pub(super) open spec fn retried(before: Node, after: Node) -> bool {
    after.attempt > before.attempt || after.phase is Planning
        || match after.question { Some(q) => matches!(q.kind, QKind::CantMeet | QKind::OutOfCredit), None => false }
}

/// Q54 / #440: one parametric comparison covers all shared work-retry diversions.
/// A free fix round is identical for both flag values and therefore does not divert.
pub(super) proof fn diversion(n: Node, off: (Node, u64), on: (Node, u64),
    id: u64, kind: u8, step: ReviewStop)
    requires kind < 4, n.generation < u64::MAX, id < u64::MAX,
        !(n.phase is Stopping || n.phase is Closed),
        kind == 2 ==> n.phase is Planning && step == ReviewStop::PlanCantMeet,
        retry_post(n, off, false, id, kind, step),
        retry_post(n, on, true, id, kind, step),
    ensures {
        on != off ==> on.0.phase == Phase::Stopping(Outcome::Stopped)
            && on.0.review_stop == Some(step) && on.0.credit == n.credit
            && on.0.question.is_none() && on.1 == id
            && on.0.attempt == n.attempt && on.0.replanned == n.replanned
            && retried(n, off.0)
    },
{
    reveal(port_failfast_diversion::plan_fields);
    reveal(port_review_retry::rung);
    reveal(port_review_retry::room);
    reveal(port_review_retry::round);
    reveal(retirement::stopped);
}

/// Q54 / #440 maker-files funding exception: an unfunded fitting extension asks
/// for maker credit under fail_fast; the ordinary exhaustion branch asks to split.
pub(super) proof fn funding(n: Node, joined: Footprint, id: u64)
    requires n.phase == Phase::Making, !port_review_retry::funded(n),
        n.review_stop.is_none(), n.generation < u64::MAX, id < u64::MAX,
    ensures {
        let on = port_review_retry::asked(Node { footprint: joined, ..n }, id,
            QKind::OutOfCredit, Cont::Ready(Resume::Making));
        let off = port_review_retry::room(n, false, id, ReviewStop::MakerNeedsFiles);
        on.phase == Phase::AwaitingRob && on.credit == n.credit && on.review_stop.is_none()
            && on.question == Some(Question { id, kind: QKind::OutOfCredit,
                cont: Cont::Ready(Resume::Making), cost: 1, clarified: false })
            && off.0.question == Some(Question { id, kind: QKind::OutOfCredit,
                cont: Cont::Planning { must_split: true }, cost: 1, clarified: false })
            && off.1 as int == id as int + 1 && retried(n, off.0)
            && on.attempt == n.attempt && on.replanned == n.replanned
    },
{
    reveal(port_review_retry::room);
}

}
