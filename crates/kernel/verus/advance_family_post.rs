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
    && (c.slot ==> a.pos == 0 && a.paused == n.paused)
}

/// R10: plain rows of the proven Ready-waiter grant contract.
pub(super) proof fn slot_grant_rows(n: Node, a: Node, c: AdvanceContext, changed: bool)
    requires slot_grant(n, a, c, changed),
    ensures changed == c.slot,
        c.slot ==> a.holds && a.ticket.is_none()
            && a.phase == resumed_phase(match n.phase { Phase::Ready(r) => r, _ => Resume::Making })
            && a.repo == n.repo && a.footprint == n.footprint
            && a.pool == n.pool && a.run == n.run,
        !c.slot ==> a == n,
{
    reveal(slot_grant);
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


/// G5/G20: a ticketed Start without a local obstruction.
pub(super) open spec fn start_unobstructed(n: Node) -> bool {
    !(n.phase is Closed) && !n.waiting_incident && n.effect.is_none()
        && !n.paused && n.ticket.is_some()
        && matches!(current_item(n), Some(Item::Start(_)))
}

#[verifier::opaque]
pub(super) open spec fn pool_grant(n: Node, a: Node, c: AdvanceContext, changed: bool) -> bool {
    changed == c.pool
        && (c.pool ==> a.pool && a.ticket.is_none() && a.run == Some(n.generation))
        && (!c.pool ==> a == n)
}

pub(super) proof fn start_item(n: Node)
    requires start_unobstructed(n),
    ensures !(n.phase is Ready),
{
    reveal(current_item);
    reveal(items);
}


/// G5/G20: permission and resource frames for every production advance arm.
#[verifier::opaque]
pub(super) open spec fn grant_facts(b: Node, a: Node, c: AdvanceContext) -> bool {
    (a.holds && !b.holds ==> c.slot && !b.paused && b.phase is Ready && b.ticket.is_some()
        && a.ticket.is_none() && a.repo == b.repo && a.footprint == b.footprint && a.pool == b.pool)
    && (a.pool && !b.pool ==> c.pool && !b.paused && b.ticket.is_some() && a.ticket.is_none())
}

pub(super) proof fn grant_no_new(b: Node, a: Node, c: AdvanceContext)
    requires a.holds ==> b.holds, a.pool ==> b.pool,
    ensures grant_facts(b, a, c), a.holds && !b.holds ==> a.run == b.run,
{
    reveal(grant_facts);
}

pub(super) proof fn grant_from_fields(b: Node, a: Node, c: AdvanceContext)
    requires a.holds ==> b.holds,
        a.pool && !b.pool ==> c.pool && !b.paused && b.ticket.is_some() && a.ticket.is_none(),
    ensures grant_facts(b, a, c), a.holds && !b.holds ==> a.run == b.run,
{
    reveal(grant_facts);
}

pub(super) proof fn slot_grant_facts(b: Node, t: u64, c: AdvanceContext, v: (Node, u64, bool))
    requires advance_pre(b, t, 0),
        !(b.phase is Closed) && !b.waiting_incident && b.effect.is_none()
            && current_item(b) == Some(Item::Slot),
        v == slot_value(b, t, c),
    ensures grant_facts(b, v.0, c), v.0.holds && !b.holds ==> v.0.run == b.run,
{
    reveal(grant_facts);
    reveal(slot_value);
    slot_item_ready(b);
}


pub(super) open spec fn holds_item(b: Node) -> bool {
    matches!(current_item(b), Some(Item::Do(_) | Item::Await | Item::Delivery | Item::Answer))
}

/// Original G5-H item frames only. Phase-wide R11 is deferred pending ruling.
#[verifier::opaque]
pub(super) open spec fn holds_frame(b: Node, a: Node) -> bool {
    (!b.paused && matches!(current_item(b), Some(Item::Do(_))) ==> a.holds == b.holds)
    && (matches!(current_item(b), Some(Item::Await | Item::Delivery | Item::Answer))
        ==> a.holds == b.holds)
}

pub(super) proof fn holds_frame_intro(b: Node, a: Node)
    requires !b.paused && matches!(current_item(b), Some(Item::Do(_))) ==> a.holds == b.holds,
        matches!(current_item(b), Some(Item::Await | Item::Delivery | Item::Answer))
            ==> a.holds == b.holds,
    ensures holds_frame(b, a),
{
    reveal(holds_frame);
}


/// E5.6's internal carrier; product clause stays plain on scheduler::advance.
#[verifier::opaque]
pub(super) open spec fn retains_holds(b: Node, a: Node) -> bool {
    b.holds && !b.paused && (b.phase == Phase::Making || b.phase == Phase::Rebasing)
        ==> a.holds
}

pub(super) proof fn retains_holds_intro(b: Node, a: Node)
    requires b.holds && !b.paused && (b.phase == Phase::Making || b.phase == Phase::Rebasing)
        ==> a.holds,
    ensures retains_holds(b, a),
{
    reveal(retains_holds);
}

/// Only these agenda arms can occur while Making or Rebasing.
pub(super) proof fn holds_phase_items(n: Node)
    requires n.phase == Phase::Making || n.phase == Phase::Rebasing,
    ensures match current_item(n) {
        Some(item) => matches!(item, Item::Do(_) | Item::Start(_) | Item::StartRebase | Item::Await),
        None => true,
    },
{
    reveal(current_item);
    reveal(items);
    match n.phase {
        Phase::Making => {
            if n.pos < 3 {
                if n.pos == 0 { assert(current_item(n) == Some(Item::Do(EffectKind::PrepareWorktree))); }
                else if n.pos == 1 { assert(current_item(n) == Some(Item::Start(Role::Make))); }
                else { assert(n.pos == 2); assert(current_item(n) == Some(Item::Await)); }
            }
        },
        Phase::Rebasing => {
            if n.pos < 2 {
                if n.pos == 0 { assert(current_item(n) == Some(Item::StartRebase)); }
                else { assert(n.pos == 1); assert(current_item(n) == Some(Item::Await)); }
            }
        },
        _ => {},
    }
}


pub(super) open spec fn teardown_gone(n: Node, kind: EffectKind) -> bool {
    match kind { EffectKind::StopRun => n.run.is_none(), EffectKind::WithdrawJob => n.job.is_none(),
        EffectKind::ClosePr => !n.pr_open, EffectKind::RemoveWorktree => !n.worktree, _ => false }
}

#[verifier::opaque]
pub(super) open spec fn teardown_rows(b: Node, a: Node, moved: bool) -> bool {
(((b.phase is Stopping && !b.waiting_incident && b.effect.is_none() && b.question.is_none()) && (match current_item(b) { Some(Item::Teardown(k)) => teardown_gone(b, k), _ => false }))
    ==> moved && a.pos == b.pos + 1 && a.phase == b.phase && a.effect.is_none() && a.holds == b.holds
        && a.run == b.run && a.job == b.job && a.pr_open == b.pr_open && a.worktree == b.worktree && a.next_seq == b.next_seq
        && a.pool == (b.pool && current_item(b) != Some(Item::Teardown(EffectKind::StopRun))))
    && (((b.phase is Stopping && !b.waiting_incident && b.effect.is_none() && b.question.is_none()) && (match current_item(b) { Some(Item::Teardown(k)) => !teardown_gone(b, k), _ => false }))
    ==> moved && a.effect.is_some() && a.effect.unwrap().seq == b.next_seq
        && current_item(b) == Some(Item::Teardown(a.effect.unwrap().kind)) && !a.effect.unwrap().cancelling
        && a.next_seq == b.next_seq + 1 && a.pos == b.pos && a.phase == b.phase && a.holds == b.holds
        && a.pool == b.pool && a.run == b.run && a.job == b.job && a.pr_open == b.pr_open && a.worktree == b.worktree)
    && (((b.phase is Stopping && !b.waiting_incident && b.effect.is_none() && b.question.is_none()) && current_item(b) == Some(Item::Release))
    ==> moved && !a.holds && a.pos == b.pos + 1 && a.phase == b.phase && a.effect.is_none()
        && a.pool == b.pool && a.run == b.run && a.job == b.job && a.pr_open == b.pr_open && a.worktree == b.worktree && a.next_seq == b.next_seq)
    && (((b.phase is Stopping && !b.waiting_incident && b.effect.is_none() && b.question.is_none()) && current_item(b).is_none())
    ==> moved && a.holds == b.holds && a.pos == 0
        && (match b.phase { Phase::Stopping(o) => a.phase == Phase::Closed(o), _ => true }))
    && (((b.phase is Stopping && !b.waiting_incident && b.effect.is_none() && b.question.is_none()) && current_item(b).is_some())
    ==> a.run_failures == b.run_failures && a.run_incidents == b.run_incidents
        && a.fx_failures == b.fx_failures && a.fx_incidents == b.fx_incidents
        && a.question == b.question && a.credit == b.credit
        && a.generation == b.generation && a.ticket == b.ticket)
}

pub(super) proof fn teardown_rows_intro(b: Node, a: Node, moved: bool)
    requires ((b.phase is Stopping && !b.waiting_incident && b.effect.is_none() && b.question.is_none()) && (match current_item(b) { Some(Item::Teardown(k)) => teardown_gone(b, k), _ => false }))
    ==> moved && a.pos == b.pos + 1 && a.phase == b.phase && a.effect.is_none() && a.holds == b.holds
        && a.run == b.run && a.job == b.job && a.pr_open == b.pr_open && a.worktree == b.worktree && a.next_seq == b.next_seq
        && a.pool == (b.pool && current_item(b) != Some(Item::Teardown(EffectKind::StopRun))),
        ((b.phase is Stopping && !b.waiting_incident && b.effect.is_none() && b.question.is_none()) && (match current_item(b) { Some(Item::Teardown(k)) => !teardown_gone(b, k), _ => false }))
    ==> moved && a.effect.is_some() && a.effect.unwrap().seq == b.next_seq
        && current_item(b) == Some(Item::Teardown(a.effect.unwrap().kind)) && !a.effect.unwrap().cancelling
        && a.next_seq == b.next_seq + 1 && a.pos == b.pos && a.phase == b.phase && a.holds == b.holds
        && a.pool == b.pool && a.run == b.run && a.job == b.job && a.pr_open == b.pr_open && a.worktree == b.worktree,
        ((b.phase is Stopping && !b.waiting_incident && b.effect.is_none() && b.question.is_none()) && current_item(b) == Some(Item::Release))
    ==> moved && !a.holds && a.pos == b.pos + 1 && a.phase == b.phase && a.effect.is_none()
        && a.pool == b.pool && a.run == b.run && a.job == b.job && a.pr_open == b.pr_open && a.worktree == b.worktree && a.next_seq == b.next_seq,
        ((b.phase is Stopping && !b.waiting_incident && b.effect.is_none() && b.question.is_none()) && current_item(b).is_none())
    ==> moved && a.holds == b.holds && a.pos == 0
        && (match b.phase { Phase::Stopping(o) => a.phase == Phase::Closed(o), _ => true }),
        ((b.phase is Stopping && !b.waiting_incident && b.effect.is_none() && b.question.is_none()) && current_item(b).is_some())
    ==> a.run_failures == b.run_failures && a.run_incidents == b.run_incidents
        && a.fx_failures == b.fx_failures && a.fx_incidents == b.fx_incidents
        && a.question == b.question && a.credit == b.credit
        && a.generation == b.generation && a.ticket == b.ticket,
    ensures teardown_rows(b, a, moved),
{
    reveal(teardown_rows);
}

pub(super) proof fn stopping_items(n: Node)
    requires n.phase is Stopping,
    ensures match current_item(n) { Some(item) => matches!(item, Item::Teardown(_) | Item::Release), None => true },
{
    reveal(current_item); reveal(items);
}

}
