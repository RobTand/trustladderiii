//! #806: complete checker frames for schema-rejected worker replies.
use super::port_replies_contracts::ended;
use super::port_replies_frames::node_change;
use super::*;

verus! {
/// Q50 failed-run row / #806: one failed operation restarts at its operation
/// entry, preserving credit, locks, questions and every other node field.
pub(super) proof fn rejected_once(w: World, n: Node, next: Node, reply: Reply)
    requires retirement::node_rules(w, n), node_headroom(n),
        current_item(n) == Some(Item::Await), !(n.phase is Stopping), n.run_failures == 0,
        next == (Node { pos: if n.phase == Phase::Making { 1u8 } else { 0u8 },
            generation: (n.generation + 1) as u64, run_failures: 1,
            run_incidents: n.run_incidents, ..ended(n) }),
    ensures retirement::node_rules(w, next), node_change(n, next, w.fail_fast, reply),
        next.credit == n.credit, next.holds == n.holds, !next.pool,
        next.repo == n.repo, next.footprint == n.footprint, next.ticket.is_none(),
{
    reveal(retirement::node_rules);
}

/// Q50 failed-run row / #806: the second failure releases a holding phase,
/// raises the bounded incident and never spends or changes a work-retry decision.
pub(super) proof fn rejected_incident(w: World, n: Node, next: Node, reply: Reply)
    requires retirement::node_rules(w, n), node_headroom(n),
        current_item(n) == Some(Item::Await), !(n.phase is Stopping), n.run_failures == 1,
        next == retirement::incident(Node {
            pos: if n.phase == Phase::Making { 1u8 } else { 0u8 },
            generation: (n.generation + 1) as u64, run_failures: 0,
            run_incidents: if n.run_incidents < 2 { (n.run_incidents + 1) as u8 } else { 2u8 },
            ..ended(n) }, n.run_incidents >= 1),
    ensures retirement::node_rules(w, next), node_change(n, next, w.fail_fast, reply),
        next.credit == n.credit, !next.holds, !next.pool,
        next.repo == n.repo, next.footprint == n.footprint, next.ticket.is_none(),
{
    reveal(retirement::node_rules); reveal(retirement::incident);
}
}
