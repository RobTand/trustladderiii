//! G5: exact Slot-arm update, isolated from world-level safety reasoning.
use super::*;

verus! {
pub(super) open spec fn same(b: Node, n: Node, t0: u64, t1: u64,
    o0: Outbox, o1: Outbox, moved: bool) -> bool {
    n == b && t1 == t0 && !moved
        && o1.len == o0.len && o1.items@ == o0.items@
}

pub(super) open spec fn issued(b: Node, n: Node, t0: u64, t1: u64,
    o0: Outbox, o1: Outbox, moved: bool) -> bool {
    n == (Node { ticket: Some(t0), ..b }) && t1 as int == t0 as int + 1
        && moved && o1.len == o0.len && o1.items@ == o0.items@
}

#[verifier::opaque]
pub(super) open spec fn slot(b: Node, n: Node, t0: u64, t1: u64, c: AdvanceContext,
    o0: Outbox, o1: Outbox, moved: bool) -> bool {
    match b.phase {
        Phase::Ready(r) => if b.paused {
            moved == b.ticket.is_some() && n == (Node { ticket: None, ..b })
                && t1 == t0 && o1.len == o0.len && o1.items@ == o0.items@
        } else if b.ticket.is_none() {
            issued(b, n, t0, t1, o0, o1, moved)
        } else if c.slot {
            moved && t1 == t0 && o1.len == o0.len && o1.items@ == o0.items@
                && n == retirement::phased(Node { holds: true, ..b }, resumed_phase(r))
        } else { same(b, n, t0, t1, o0, o1, moved) },
        _ => same(b, n, t0, t1, o0, o1, moved),
    }
}


pub(super) proof fn slot_paused(b: Node, n: Node, t0: u64, t1: u64,
    c: AdvanceContext, o0: Outbox, o1: Outbox, moved: bool)
    requires b.phase is Ready, b.paused,
        n == (Node { ticket: None, ..b }), moved == b.ticket.is_some(),
        t1 == t0, o1.len == o0.len, o1.items@ == o0.items@,
    ensures slot(b, n, t0, t1, c, o0, o1, moved),
{
    reveal(slot);
}

pub(super) proof fn slot_ticket_issued(b: Node, n: Node, t0: u64, t1: u64,
    c: AdvanceContext, o0: Outbox, o1: Outbox, moved: bool)
    requires b.phase is Ready, !b.paused, b.ticket.is_none(),
        n == (Node { ticket: Some(t0), ..b }), t1 as int == t0 as int + 1,
        moved, o1.len == o0.len, o1.items@ == o0.items@,
    ensures slot(b, n, t0, t1, c, o0, o1, moved),
{
    reveal(slot);
}

pub(super) proof fn slot_granted(b: Node, n: Node, t0: u64, t1: u64,
    c: AdvanceContext, o0: Outbox, o1: Outbox, moved: bool, r: Resume)
    requires b.phase == Phase::Ready(r), !b.paused, b.ticket.is_some(), c.slot,
        n == retirement::phased(Node { holds: true, ..b }, resumed_phase(r)),
        t1 == t0, moved, o1.len == o0.len, o1.items@ == o0.items@,
    ensures slot(b, n, t0, t1, c, o0, o1, moved),
{
    reveal(slot);
}

pub(super) proof fn slot_unchanged(b: Node, n: Node, t0: u64, t1: u64,
    c: AdvanceContext, o0: Outbox, o1: Outbox, moved: bool)
    requires !(b.phase is Ready) || (!b.paused && b.ticket.is_some() && !c.slot),
        n == b, t1 == t0, !moved, o1.len == o0.len, o1.items@ == o0.items@,
    ensures slot(b, n, t0, t1, c, o0, o1, moved),
{
    reveal(slot);
}

/// G5 #806: value result of each Slot arm, as the executable helpers build it.
pub(super) open spec fn slot_paused_value(n: Node, t: u64) -> (Node, u64, bool) {
    if n.ticket.is_some() { (Node { ticket: None, ..n }, t, true) } else { (n, t, false) }
}

pub(super) open spec fn slot_issue_value(n: Node, t: u64) -> (Node, u64, bool) {
    (Node { ticket: Some(t), ..n }, (t + 1) as u64, true)
}

pub(super) open spec fn slot_grant_value(n: Node, t: u64, r: Resume) -> (Node, u64, bool) {
    (retirement::phased(Node { holds: true, ..n }, resumed_phase(r)), t, true)
}

/// G5 #806: advance_slot's executable result. Each arm is, by definition, one helper's post.
#[verifier::opaque]
pub(super) open spec fn slot_value(n: Node, t: u64, c: AdvanceContext) -> (Node, u64, bool) {
    match n.phase {
        Phase::Ready(r) => if n.paused { slot_paused_value(n, t) }
            else if n.ticket.is_none() { slot_issue_value(n, t) }
            else if c.slot { slot_grant_value(n, t, r) }
            else { (n, t, false) },
        _ => (n, t, false),
    }
}

/// G5: Slot is Ready's only item, so the Slot entry sees Ready at position 0.
pub(super) proof fn slot_item_ready(b: Node)
    requires current_item(b) == Some(Item::Slot),
    ensures b.phase is Ready, b.pos == 0,
{
    reveal(current_item);
    reveal(items);
}

/// G5: the arithmetic headroom the issuing and granting arms consume.
pub(super) proof fn slot_bounds(b: Node, t0: u64, out_len: usize)
    requires advance_pre(b, t0, out_len),
        !(b.phase is Closed) && !b.waiting_incident && b.effect.is_none()
            && current_item(b) == Some(Item::Slot),
    ensures b.phase is Ready && !b.paused ==> b.generation < u64::MAX && t0 < u64::MAX,
{
    slot_item_ready(b);
}

/// G5 #806: the Slot entry's posts for the paused arm, over its value result.
pub(super) proof fn slot_facts_paused(b: Node, t0: u64, c: AdvanceContext, o0: Outbox,
    e: Seq<report::ReportEntry>)
    requires advance_pre(b, t0, o0.len),
        !(b.phase is Closed) && !b.waiting_incident && b.effect.is_none()
            && current_item(b) == Some(Item::Slot),
        b.phase is Ready, b.paused,
    ensures ({
        let v = slot_paused_value(b, t0);
        advance_post(b, v.0, t0, v.1, v.2) && v.2 == advance_enabled(b, c)
            && slot(b, v.0, t0, v.1, c, o0, o0, v.2)
            && report::no_new_stop(b.phase, v.0.phase, e, e) && t0 <= v.1
    }),
{
    reveal(advance_post);
    reveal(distance);
    let v = slot_paused_value(b, t0);
    ranks::pending_rank_definition(b);
    ranks::pending_rank_definition(v.0);
    assert(v.0 == (Node { ticket: None, ..b }));
    slot_paused(b, v.0, t0, v.1, c, o0, o0, v.2);
    report::lemmas::quiet(b.phase, v.0.phase, e);
}

/// G5 #806: the Slot entry's posts for the granting arm, over its value result.
pub(super) proof fn slot_facts_grant(b: Node, t0: u64, c: AdvanceContext, o0: Outbox,
    e: Seq<report::ReportEntry>, r: Resume)
    requires advance_pre(b, t0, o0.len),
        !(b.phase is Closed) && !b.waiting_incident && b.effect.is_none()
            && current_item(b) == Some(Item::Slot),
        b.phase == Phase::Ready(r), !b.paused, b.ticket.is_some(), c.slot,
    ensures ({
        let v = slot_grant_value(b, t0, r);
        advance_post(b, v.0, t0, v.1, v.2) && v.2 == advance_enabled(b, c)
            && slot(b, v.0, t0, v.1, c, o0, o0, v.2)
            && report::no_new_stop(b.phase, v.0.phase, e, e) && t0 <= v.1
    }),
{
    reveal(advance_post);
    reveal(distance);
    let v = slot_grant_value(b, t0, r);
    ranks::pending_rank_definition(b);
    ranks::pending_rank_definition(v.0);
    slot_item_ready(b);
    // One resume target per case keeps items/current_item of the new phase ground.
    match r {
        Resume::Making => {}, Resume::Checking => {}, Resume::Reviewing => {},
        Resume::Publishing => {}, Resume::Rebasing => {},
    }
    slot_granted(b, v.0, t0, v.1, c, o0, o0, v.2, r);
    report::lemmas::quiet(b.phase, v.0.phase, e);
}

pub(super) proof fn slot_facts_issue(b: Node, t0: u64, c: AdvanceContext, o0: Outbox,
    e: Seq<report::ReportEntry>)
    requires advance_pre(b, t0, o0.len),
        !(b.phase is Closed) && !b.waiting_incident && b.effect.is_none()
            && current_item(b) == Some(Item::Slot),
        b.phase is Ready, !b.paused, b.ticket.is_none(),
    ensures ({
        let v = slot_issue_value(b, t0);
        advance_post(b, v.0, t0, v.1, v.2) && v.2 == advance_enabled(b, c)
            && slot(b, v.0, t0, v.1, c, o0, o0, v.2)
            && report::no_new_stop(b.phase, v.0.phase, e, e) && t0 <= v.1
    }),
{
    reveal(advance_post);
    reveal(distance);
    let v = slot_issue_value(b, t0);
    ranks::pending_rank_definition(b);
    ranks::pending_rank_definition(v.0);
    slot_item_ready(b);
    slot_bounds(b, t0, o0.len);
    slot_ticket_issued(b, v.0, t0, v.1, c, o0, o0, v.2);
    report::lemmas::quiet(b.phase, v.0.phase, e);
}

pub(super) proof fn slot_facts_wait(b: Node, t0: u64, c: AdvanceContext, o0: Outbox,
    e: Seq<report::ReportEntry>)
    requires advance_pre(b, t0, o0.len),
        !(b.phase is Closed) && !b.waiting_incident && b.effect.is_none()
            && current_item(b) == Some(Item::Slot),
        b.phase is Ready, !b.paused, b.ticket.is_some(), !c.slot,
    ensures ({
        let v = (b, t0, false);
        advance_post(b, v.0, t0, v.1, v.2) && v.2 == advance_enabled(b, c)
            && slot(b, v.0, t0, v.1, c, o0, o0, v.2)
            && report::no_new_stop(b.phase, v.0.phase, e, e) && t0 <= v.1
    }),
{
    reveal(advance_post);
    reveal(distance);
    slot_unchanged(b, b, t0, t0, c, o0, o0, false);
    report::lemmas::quiet(b.phase, b.phase, e);
}

/// G5 #806: the bridge. advance_slot_value's result satisfies every advance_slot post.
pub(super) proof fn slot_bridge(b: Node, t0: u64, c: AdvanceContext, o0: Outbox,
    e: Seq<report::ReportEntry>, v: (Node, u64, bool))
    requires advance_pre(b, t0, o0.len),
        !(b.phase is Closed) && !b.waiting_incident && b.effect.is_none()
            && current_item(b) == Some(Item::Slot),
        v == slot_value(b, t0, c),
    ensures advance_post(b, v.0, t0, v.1, v.2),
        v.2 == advance_enabled(b, c),
        slot(b, v.0, t0, v.1, c, o0, o0, v.2),
        report::no_new_stop(b.phase, v.0.phase, e, e),
        t0 <= v.1,
{
    reveal(slot_value);
    slot_item_ready(b);
    match b.phase {
        Phase::Ready(r) => {
            if b.paused { slot_facts_paused(b, t0, c, o0, e); }
            else if b.ticket.is_none() { slot_facts_issue(b, t0, c, o0, e); }
            else if c.slot { slot_facts_grant(b, t0, c, o0, e, r); }
            else { slot_facts_wait(b, t0, c, o0, e); }
        },
        _ => {},
    }
}

/// G20/G22: a Ready waiter with a ticket and no local obstruction.
pub(super) open spec fn ready_unobstructed(n: Node) -> bool {
    n.phase is Ready && n.pos == 0 && !n.paused && !n.waiting_incident
        && n.effect.is_none() && n.ticket.is_some()
}

/// G20/G22: an unobstructed Ready waiter is granted exactly when the context offers the slot.
#[verifier::opaque]
pub(super) open spec fn slot_grant(n: Node, a: Node, c: AdvanceContext, changed: bool) -> bool {
    changed == c.slot
    && (c.slot ==> a.holds && a.ticket.is_none()
        && a.phase == resumed_phase(match n.phase { Phase::Ready(r) => r, _ => Resume::Making })
        && a.repo == n.repo && a.footprint == n.footprint
        && a.pool == n.pool && a.run == n.run)
    && (!c.slot ==> a == n)
}

pub(super) proof fn slot_grant_from_value(b: Node, t0: u64, c: AdvanceContext, v: (Node, u64, bool))
    requires ready_unobstructed(b), v == slot_value(b, t0, c),
    ensures slot_grant(b, v.0, c, v.2),
{
    reveal(slot_grant);
    reveal(slot_value);
}

/// G20/G22: an unobstructed Ready node dispatches to Slot.
pub(super) proof fn ready_item(n: Node)
    requires ready_unobstructed(n),
    ensures current_item(n) == Some(Item::Slot),
{
    reveal(current_item);
    reveal(items);
}

/// K5 frozen export E5 (CONTRACTS.md; owner Advance; consumers assembly G5/G6/G19/G20, liveness G22).
/// TRUSTED at fv-k5-int-dev-1110: `scheduler::advance` ensures it through a trusted wrapper. One opaque
/// bundle so that advance's proof callers see a single atom; consumers `reveal(advance_exports)`.
#[verifier::opaque]
pub(super) open spec fn advance_exports(before: World, after: World, i: usize, moved: bool,
    ob: Outbox, oa: Outbox) -> bool {
    let k = i as int;
    let b = before.nodes@[k];
    let a = after.nodes@[k];
    // E5.1 One advance is an internal step the checker composition accepts (internal; feeds R5, R6).
    ((checking::accepted(before) && before.gates@.len() <= u64::MAX && before.nodes@.len() <= u64::MAX)
        ==> internal_safety::step_premises(before, after, k)
            && (b.phase is Closed ==> a.phase is Closed)
            && (a.credit.withheld ==> b.credit.withheld
                || !dependencies::prerequisites_delivered(before, NodeId(i as u64)))
            && (dependencies::prerequisites_delivered(before, NodeId(i as u64))
                || dependencies::owner_waiting_ok(a)))
    // E5.2 The Advance arm of the transition relation, exact stutter included (internal).
    && transition_safety::transition_safety(before, after,
        transition_safety::Transition::Advance { node: i, moved }, transition_safety::emitted(ob, oa))
    // E5.3 A new pool place needs pool room and first place in the pool line (REQUIREMENT R7).
    && (!b.pool && a.pool ==> pool_users(before.nodes@, before.nodes@.len()) < before.pool_capacity
        && oldest_waiter(before, k))
    // E5.4 A runnable start that is first in the pool line with room is granted, and leaves the line (REQUIREMENT R8).
    && (((match current_item(b) { Some(Item::Start(_)) => true, _ => false })
        && !(b.phase is Closed) && !b.waiting_incident && b.effect.is_none() && !b.paused
        && b.ticket.is_some()
        && pool_users(before.nodes@, before.nodes@.len()) < before.pool_capacity && oldest_waiter(before, k))
        ==> moved && a.pool && a.ticket.is_none())
    // E5.5 A new gate slot only for an unpaused node with room and no overlapping holder; it keeps its
    // repository, files, pool place and run, and leaves the line (REQUIREMENTS R9, R10).
    && (!b.holds && a.holds ==> !b.paused && scheduler::slot_spec(before, k)
        && a.repo == b.repo && a.footprint == b.footprint && a.ticket.is_none()
        && a.pool == b.pool && a.run == b.run)
    // E5.6 A slot holder that is making its change or rebasing keeps its slot (REQUIREMENT R11).
    && (b.holds && (b.phase == Phase::Making || b.phase == Phase::Rebasing) ==> a.holds)
}

}
