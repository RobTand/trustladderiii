//! Q30/Q39/Q48 / #806: exact production gate scan predicates and grant safety.
use super::*;

verus! {
/// State-table gate occupancy: the actual slot scan excludes the requesting index.
pub(super) open spec fn slot_count(w: World, i: int, end: nat) -> nat
    decreases end,
{
    if end == 0 { 0 } else {
        let j = end as int - 1;
        slot_count(w, i, (end - 1) as nat)
            + if j != i && w.nodes@[j].holds && w.nodes@[j].repo == w.nodes@[i].repo { 1nat } else { 0nat }
    }
}

/// Q30: a slot requires capacity and every held footprint to be disjoint.
pub(super) open spec fn slot(w: World, i: int) -> bool {
    let n = w.nodes@[i];
    n.repo.0 <= usize::MAX && n.repo.0 < w.gates@.len()
        && slot_count(w, i, w.nodes@.len()) < w.gates@[n.repo.0 as int].w
        && forall|j: int| 0 <= j < w.nodes@.len() && j != i
            && w.nodes@[j].holds && w.nodes@[j].repo == n.repo
            ==> footprint::disjoint(w, w.nodes@[j].footprint, n.footprint)
}

/// Q48: the real inner scan includes all older Ready tickets, including paused ones.
pub(super) open spec fn older(w: World, i: int, j: int) -> bool {
    w.nodes@[j].phase is Ready && w.nodes@[j].repo == w.nodes@[i].repo
        && match (w.nodes@[j].ticket, w.nodes@[i].ticket) {
            (Some(theirs), Some(mine)) => theirs < mine, _ => false,
        }
}

/// Q48: any earlier overlapping ticket blocks this waiter in the real inner scan.
pub(super) open spec fn overlap(w: World, i: int, ticket: u64) -> bool {
    exists|j: int| overlapping_before(w, i, j, ticket)
}

/// Q48: one concrete earlier-overlap witness in the executable scan.
pub(super) open spec fn overlapping_before(w: World, i: int, j: int, ticket: u64) -> bool {
    0 <= j < w.nodes@.len() && w.nodes@[j].phase is Ready
        && w.nodes@[j].repo == w.nodes@[i].repo
        && (match w.nodes@[j].ticket { Some(theirs) => theirs < ticket, None => false })
        && !footprint::disjoint(w, w.nodes@[j].footprint, w.nodes@[i].footprint)
}

/// Q48: the exact check performed for each older Ready waiter.
pub(super) open spec fn eligible(w: World, i: int, j: int) -> bool {
    footprint::disjoint(w, w.nodes@[j].footprint, w.nodes@[i].footprint)
        && (!slot(w, j) || match w.nodes@[j].ticket { Some(t) => overlap(w, j, t), None => false })
}

/// Q48: exact Ready-line predicate. A runnable older ticket wins unless its own
/// earlier overlap blocks it; a directly overlapping older ticket always wins.
pub(super) open spec fn line_prefix(w: World, i: int, end: int) -> bool {
    forall|j: int| 0 <= j < end && #[trigger] older(w, i, j) ==> eligible(w, i, j)
}

/// Q48: a ticket is necessary; the entire prefix must admit this Ready waiter.
pub(super) open spec fn line(w: World, i: int) -> bool {
    w.nodes@[i].ticket.is_some() && line_prefix(w, i, w.nodes@.len() as int)
}

/// Q48: being first in line excludes every earlier overlap.
pub(super) proof fn line_excludes_overlap(w: World, i: int, ticket: u64)
    requires 0 <= i < w.nodes@.len(), w.nodes@[i].ticket == Some(ticket), line(w, i),
    ensures !overlap(w, i, ticket),
{
    if overlap(w, i, ticket) {
        let j = choose|j: int| overlapping_before(w, i, j, ticket);
        assert(older(w, i, j));
        assert(eligible(w, i, j));
    }
}

/// Q48: the production scan extends its first-in-line condition one index at a time.
pub(super) proof fn prefix_step(w: World, i: int, end: int)
    requires 0 <= i < w.nodes@.len(), 0 <= end < w.nodes@.len(),
    ensures line_prefix(w, i, end + 1) == (line_prefix(w, i, end)
        && (older(w, i, end) ==> eligible(w, i, end))),
{
    if line_prefix(w, i, end) && (older(w, i, end) ==> eligible(w, i, end)) {
        assert forall|j: int| 0 <= j < end + 1 && older(w, i, j) implies eligible(w, i, j) by {
            if j < end { assert(eligible(w, i, j)); }
        }
    }
    if line_prefix(w, i, end + 1) {
        assert forall|j: int| 0 <= j < end && older(w, i, j) implies eligible(w, i, j) by {}
        assert(older(w, i, end) ==> eligible(w, i, end));
    }
}
}

verus! {
/// Q30/Q39: exact local Ready update, connected to advance_slot's executable body.
#[verifier::opaque]
pub(super) open spec fn slot_node(n: Node, ticket: u64, grant: bool) -> Node {
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
pub(super) open spec fn slot_ticket(n: Node, ticket: u64) -> u64 {
    if !n.paused && n.ticket.is_none() { (ticket + 1) as u64 } else { ticket }
}

/// Q30/Q39: changed is exact even when a paused ticket is merely cleared.
pub(super) open spec fn slot_changed(n: Node, grant: bool) -> bool {
    if n.paused { n.ticket.is_some() } else { n.ticket.is_none() || grant }
}

/// Q30/Q48: input shape of the original four-node grant harness, with weaker
/// bounds and arbitrary other nodes satisfying full production consistency.
pub(super) open spec fn grant_input(w: World) -> bool {
    retirement_wide::domain(w) && w.nodes@.len() == 4 && w.gates@.len() == 1 && w.file_sets@.len() == 0
        && retirement_wide::accepted(w) && retirement::bounded(w)
        && w.nodes@[0].phase == Phase::Ready(Resume::Making)
        && w.nodes@[0].effect.is_none() && !w.nodes@[0].waiting_incident
}
}

verus! {
/// Q30/Q39/Q48 / #806: a real Ready(Making) move preserves complete consistency.
/// Bound: four original nodes, one repository; footprints are arbitrary inline sets.
pub(super) proof fn grant_safe(before: World, after: World)
    requires grant_input(before),
        after.nodes@ == before.nodes@.update(0, slot_node(before.nodes@[0], before.next_ticket, slot(before, 0) && line(before, 0))),
        after.next_ticket == slot_ticket(before.nodes@[0], before.next_ticket),
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@, after.next_question == before.next_question,
        after.pool_capacity == before.pool_capacity,
    ensures retirement_wide::domain(after), retirement_wide::accepted(after),
        retirement_retry::node_post(before.nodes@[0], after.nodes@[0], before.fail_fast),
        after.nodes@[0].holds ==> slot(before, 0) && line(before, 0)
            && !before.nodes@[0].paused && after.nodes@[0].ticket.is_none(),
        !after.nodes@[0].holds ==> !after.nodes@[0].pool
            && (before.nodes@[0].ticket.is_some() && !before.nodes@[0].paused
                ==> after.nodes@[0].ticket == before.nodes@[0].ticket),
        forall|j: int| 0 < j < 4 && before.nodes@[j].phase is Ready && !before.nodes@[j].paused
            && older(before, 0, j) ==> {
                (slot(before, j) && line(before, j)
                    || !footprint::disjoint(before, before.nodes@[j].footprint, before.nodes@[0].footprint))
                    ==> !after.nodes@[0].holds
            }
            && (after.nodes@[0].holds ==> !(slot(before, j) && line(before, j))
                && footprint::disjoint(before, before.nodes@[j].footprint, before.nodes@[0].footprint)),
{
    reveal(retirement_wide::accepted); reveal(retirement::node_rules); reveal(slot_node);
    reveal_with_fuel(potential_prefix, 5); reveal_with_fuel(slot_count, 5);
    assert(before.next_ticket <= after.next_ticket);
    assert forall|j: int| 0 <= j < 4 implies retirement::node_rules(after, after.nodes@[j]) by {
        retirement_retry::question_frame(before, after, before.nodes@[j]);
    }
    assert forall|a: int, b: int| 0 <= a < b < 4 implies retirement_wide::pair(after, a, b) by {
        footprint::storage_frame(before, after, before.nodes@[a].footprint, before.nodes@[b].footprint);
        if a == 0 && after.nodes@[0].holds && after.nodes@[b].holds && after.nodes@[b].repo == after.nodes@[0].repo {
            assert(slot(before, 0));
            assert(footprint::disjoint(before, before.nodes@[b].footprint, before.nodes@[0].footprint));
            footprint::inline_disjoint_symmetric(before, before.nodes@[0].footprint, before.nodes@[b].footprint);
        } else {
            assert(retirement_wide::pair(before, a, b));
        }
    }
    assert forall|j: int| 0 < j < 4 && before.nodes@[j].phase is Ready && !before.nodes@[j].paused
        && older(before, 0, j) implies {
            (slot(before, j) && line(before, j)
                || !footprint::disjoint(before, before.nodes@[j].footprint, before.nodes@[0].footprint))
                ==> !after.nodes@[0].holds
        } && (after.nodes@[0].holds ==> !(slot(before, j) && line(before, j))
            && footprint::disjoint(before, before.nodes@[j].footprint, before.nodes@[0].footprint)) by {
        if after.nodes@[0].holds {
            assert(eligible(before, 0, j));
            if line(before, j) {
                line_excludes_overlap(before, j, before.nodes@[j].ticket.unwrap());
            }
        }
    }
}
}

kernel_fn! {
/// Q30/Q39/Q48 / #806: original grant harness over real advance and the real checker.
/// All original ordering and transition assertions are explicit. Width is arbitrary,
/// so both W2 and W3 are covered without a finite scheduler model.
pub(super) fn verify_grant(mut w: World) -> (result: (World, Outbox, bool)) [
    requires grant_input(w), storage_ordered(w),
    ensures retirement_wide::accepted(result.0),
        result.0.nodes@ == w.nodes@.update(0, slot_node(w.nodes@[0], w.next_ticket, slot(w, 0) && line(w, 0))),
        result.1.len == 0,
        retirement_retry::node_post(w.nodes@[0], result.0.nodes@[0], w.fail_fast),
        result.2 ==> 4 * node_distance(result.0.nodes@[0]) + pending_rank(result.0.nodes@[0])
            < 4 * node_distance(w.nodes@[0]) + pending_rank(w.nodes@[0]),
        result.0.nodes@[0].holds ==> slot(w, 0) && line(w, 0)
            && !w.nodes@[0].paused && result.0.nodes@[0].ticket.is_none(),
        !result.0.nodes@[0].holds ==> !result.0.nodes@[0].pool
            && (w.nodes@[0].ticket.is_some() && !w.nodes@[0].paused ==> result.0.nodes@[0].ticket == w.nodes@[0].ticket),
        forall|j: int| 0 < j < 4 && w.nodes@[j].phase is Ready && !w.nodes@[j].paused && older(w, 0, j) ==>
            ((slot(w, j) && line(w, j) || !footprint::disjoint(w, w.nodes@[j].footprint, w.nodes@[0].footprint))
                ==> !result.0.nodes@[0].holds)
            && (result.0.nodes@[0].holds ==> !(slot(w, j) && line(w, j))
                && footprint::disjoint(w, w.nodes@[j].footprint, w.nodes@[0].footprint)),
] proof [
    reveal(retirement_wide_domain); reveal(retirement_wide_accepted);
    reveal(retirement_wide::accepted); reveal(retirement::bounded);
    reveal(world_headroom); reveal_with_fuel(open_prefix, 5); reveal(storage_ordered);
    reveal(empty_outbox); reveal(advance_post);
] {
    kernel_snapshot! { before = w }
    kernel_proof! {
        open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
        assert forall|j: int| 0 <= j < w.nodes@.len() implies node_headroom(w.nodes@[j]) by {}
    }
    let checked_before = w.check();
    assert!(checked_before.is_ok());
    kernel_proof! { world_checked_definition(w); }
    kernel_proof! { rank_definitions(w.nodes@[0]); }
    let before_rank = w.nodes[0].settle_rank();
    let mut out = Outbox::new();
    let changed = advance(&mut w, 0, &mut out);
    kernel_proof! { rank_definitions(w.nodes@[0]); }
    let after_rank = w.nodes[0].settle_rank();
    if changed { assert!(after_rank < before_rank); }
    kernel_proof! { grant_safe(before@, w); }
    let checked = w.check();
    assert!(checked.is_ok());
    (w, out, changed)
}
}
