//! Q30/Q39/Q48 / #806: neutral exact gate contracts shared with the phase-one rebuild.
use super::*;

verus! {
/// State-table gate occupancy: the actual gate_slot scan excludes the requesting index.
pub(super) open spec fn gate_slot_count(w: World, i: int, end: nat) -> nat
    decreases end,
{
    if end == 0 { 0 } else {
        let j = end as int - 1;
        gate_slot_count(w, i, (end - 1) as nat)
            + if j != i && w.nodes@[j].holds && w.nodes@[j].repo == w.nodes@[i].repo { 1nat } else { 0nat }
    }
}

/// Q30: a gate_slot requires capacity and every held footprint to be disjoint.
pub(super) open spec fn gate_slot(w: World, i: int) -> bool {
    let n = w.nodes@[i];
    n.repo.0 <= usize::MAX && n.repo.0 < w.gates@.len()
        && gate_slot_count(w, i, w.nodes@.len()) < w.gates@[n.repo.0 as int].w
        && forall|j: int| 0 <= j < w.nodes@.len() && j != i
            && w.nodes@[j].holds && w.nodes@[j].repo == n.repo
            ==> footprint::disjoint(w, w.nodes@[j].footprint, n.footprint)
}

/// Q48: the real inner scan includes all gate_older Ready tickets, including paused ones.
pub(super) open spec fn gate_older(w: World, i: int, j: int) -> bool {
    w.nodes@[j].phase is Ready && w.nodes@[j].repo == w.nodes@[i].repo
        && match (w.nodes@[j].ticket, w.nodes@[i].ticket) {
            (Some(theirs), Some(mine)) => theirs < mine, _ => false,
        }
}

/// Q48: any earlier overlapping ticket blocks this waiter in the real inner scan.
pub(super) open spec fn gate_overlap(w: World, i: int, ticket: u64) -> bool {
    exists|j: int| gate_overlapping_before(w, i, j, ticket)
}

/// Q48: one concrete earlier-gate_overlap witness in the executable scan.
pub(super) open spec fn gate_overlapping_before(w: World, i: int, j: int, ticket: u64) -> bool {
    0 <= j < w.nodes@.len() && w.nodes@[j].phase is Ready
        && w.nodes@[j].repo == w.nodes@[i].repo
        && (match w.nodes@[j].ticket { Some(theirs) => theirs < ticket, None => false })
        && !footprint::disjoint(w, w.nodes@[j].footprint, w.nodes@[i].footprint)
}

/// Q48: the exact check performed for each gate_older Ready waiter.
pub(super) open spec fn gate_eligible(w: World, i: int, j: int) -> bool {
    footprint::disjoint(w, w.nodes@[j].footprint, w.nodes@[i].footprint)
        && (!gate_slot(w, j) || match w.nodes@[j].ticket { Some(t) => gate_overlap(w, j, t), None => false })
}

/// Q48: exact Ready-gate_line predicate. A runnable gate_older ticket wins unless its own
/// earlier gate_overlap blocks it; a directly overlapping gate_older ticket always wins.
pub(super) open spec fn gate_line_prefix(w: World, i: int, end: int) -> bool {
    forall|j: int| 0 <= j < end && #[trigger] gate_older(w, i, j) ==> gate_eligible(w, i, j)
}

/// Q48: a ticket is necessary; the entire prefix must admit this Ready waiter.
pub(super) open spec fn gate_line(w: World, i: int) -> bool {
    w.nodes@[i].ticket.is_some() && gate_line_prefix(w, i, w.nodes@.len() as int)
}

/// Q48: the production scan extends its first-in-gate_line condition one index at a time.
pub(super) proof fn gate_prefix_step(w: World, i: int, end: int)
    requires 0 <= i < w.nodes@.len(), 0 <= end < w.nodes@.len(),
    ensures gate_line_prefix(w, i, end + 1) == (gate_line_prefix(w, i, end)
        && (gate_older(w, i, end) ==> gate_eligible(w, i, end))),
{
    if gate_line_prefix(w, i, end) && (gate_older(w, i, end) ==> gate_eligible(w, i, end)) {
        assert forall|j: int| 0 <= j < end + 1 && gate_older(w, i, j) implies gate_eligible(w, i, j) by {
            if j < end { assert(gate_eligible(w, i, j)); }
        }
    }
    if gate_line_prefix(w, i, end + 1) {
        assert forall|j: int| 0 <= j < end && gate_older(w, i, j) implies gate_eligible(w, i, j) by {}
        assert(gate_older(w, i, end) ==> gate_eligible(w, i, end));
    }
}
}

verus! {
/// Q30/Q39: exact local Ready update, connected to advance_slot's executable body.
#[verifier::opaque]
pub(super) open spec fn gate_slot_node(n: Node, ticket: u64, grant: bool) -> Node {
    match n.phase {
        Phase::Ready(r) => if n.paused {
            Node { ticket: None, ..n }
        } else if n.ticket.is_none() { Node { ticket: Some(ticket), ..n } }
        else if grant { retirement::phased(Node { holds: true, ..n }, resumed_phase(r)) }
        else { n },
        _ => n,
    }
}

/// Q39: only a new unpaused waiter consumes a global ticket.
pub(super) open spec fn gate_slot_ticket(n: Node, ticket: u64) -> u64 {
    if !n.paused && n.ticket.is_none() { (ticket + 1) as u64 } else { ticket }
}

/// Q30/Q39: changed is exact even when a paused ticket is merely cleared.
pub(super) open spec fn gate_slot_changed(n: Node, grant: bool) -> bool {
    if n.paused { n.ticket.is_some() } else { n.ticket.is_none() || grant }
}


/// Q30 / Q39 / #806: one exact relation for the Ready(Making) executable grant.
pub(super) open spec fn gate_ready_post(n: Node, m: Node, before_ticket: u64,
    after_ticket: u64, grant: bool, changed: bool, before_out: Outbox, after_out: Outbox) -> bool {
    m == gate_slot_node(n, before_ticket, grant)
        && after_ticket == gate_slot_ticket(n, before_ticket)
        && after_out == before_out && changed == gate_slot_changed(n, grant)
}

/// Q30 / Q39 / #806: project the shared exact frame without re-proving advance_slot.
pub(super) proof fn gate_ready_projection(n: Node, ticket: u64, c: AdvanceContext, out: Outbox)
    ensures n.phase == Phase::Ready(Resume::Making) && n.pos == 0
        && !n.waiting_incident && n.effect.is_none() ==> forall|m: Node, t: u64, o: Outbox|
            #[trigger] port_advance_contracts::exact(n, m, ticket, t, c, out, o)
                ==> gate_ready_post(n, m, ticket, t, c.slot, advance_enabled(n, c), out, o),
{
    reveal(gate_slot_node);
    reveal(port_advance_contracts::exact); reveal(port_advance_contracts::node_after);
    reveal(port_advance_contracts::outputs); reveal(port_advance_contracts::effect_kind);
}
}
