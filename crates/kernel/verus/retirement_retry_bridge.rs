//! #806: preserve the reviewed retirement predicates through W2's shared exact contracts.
use super::*;
verus! {
/// Q54 / #806: the shared exact rung tuple supplies the reviewed node and allocation result.
pub(super) proof fn rung(n: Node, fast: bool, id: u64, step: ReviewStop)
    requires id < u64::MAX,
    ensures port_review_retry::rung(n, fast, id, step).0 == retirement_retry::rung(n, fast, id, step),
        port_review_retry::rung(n, fast, id, step).1 as int == id as int
            + if retirement_retry::rung_asks(n, fast) { 1int } else { 0int },
{
    reveal(retirement_retry::rung); reveal(port_review_retry::rung);
}
/// Q54 / #806: a free first fix retains the same reviewed credit ladder.
pub(super) proof fn round(n: Node, fast: bool, id: u64, step: ReviewStop)
    requires id < u64::MAX,
    ensures port_review_retry::round(n, fast, id, step).0 == retirement_retry::failed(n, fast, id, step),
        port_review_retry::round(n, fast, id, step).1 as int == id as int
            + if n.round != 0 && retirement_retry::rung_asks(n, fast) { 1int } else { 0int },
{
    reveal(retirement_retry::failed); reveal(port_review_retry::round);
    rung(n, fast, id, step);
}
/// Q50 / #806: the shared saturated-recheck tuple preserves the reviewed exact update.
pub(super) proof fn recheck(n: Node, target: Phase, id: u64)
    requires id < u64::MAX,
    ensures port_review_retry::rechecked(n, target, id).0 == retirement_retry::rechecked(n, target, id),
        port_review_retry::rechecked(n, target, id).1 as int == id as int
            + if n.rechecks >= FREE_RECHECKS && !retirement_retry::can_pay(n) { 1int } else { 0int },
{
    reveal(retirement_retry::rechecked); reveal(port_review_retry::rechecked); reveal(retirement_retry::recheck_core);
    reveal(retirement_retry::continued);
}
/// Q50 / #806 / #860: W2's exact failed run and #860's failed node agree in run_failed's domain.
pub(super) proof fn failed_node(n: Node)
    requires n.run_failures <= 1, n.run_incidents <= 2,
    ensures port_review_retry::failed_run(n) == port_t860_contracts::failed_node(n),
{
    reveal(port_review_retry::failed_run); reveal(port_t860_contracts::failed_node);
}
}
