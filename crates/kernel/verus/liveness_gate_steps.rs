//! K5 G22: the step relations the gate-leaf trace composes, one per real harness call.
//! `advanced`, `advanced_grants`, `advanced_teardown`, `stopped` and `acked` restate the exported
//! contracts of scheduler::advance, on_command and on_ack on fv-k5-int-dev-1110 v2.10 `391658dc`
//! (existing frames, E3.1, E3.2, E4.1, E5.1, E5.3, E5.5, E5.6 v2.8, E11.1, E11.2, E11.3) clause for
//! clause, over (old(w), final(w)). `advanced_requested` and `teardown_requested` are NOT exported:
//! they are the clauses kernel/k5/liveness/opus/CONTRACT-REQUEST.md asks for.
use super::*;

verus! {
/// scheduler::advance: existing world frames and the E5.1/E5.3/E5.5 clauses acceptance needs.
#[verifier::opaque]
pub(super) open spec fn advanced(b: World, a: World, i: int, r: bool) -> bool {
    &&& 0 <= i < b.nodes@.len()
    &&& a.nodes@ == b.nodes@.update(i, a.nodes@[i])
    &&& a.next_question == b.next_question
    &&& a.gates@ == b.gates@
    &&& a.pool_capacity == b.pool_capacity
    &&& a.file_sets@ == b.file_sets@
    &&& a.dependencies@ == b.dependencies@
    &&& b.next_ticket <= a.next_ticket
    &&& ((forall|j: int| 0 <= j < b.nodes@.len() ==> settle_shape(#[trigger] b.nodes@[j]))
        ==> scheduling_safe(a)
            && (forall|j: int| 0 <= j < a.nodes@.len() ==> settle_shape(#[trigger] a.nodes@[j])))
    &&& (checking::accepted(b) ==> advance_family_post::step_rows(b, a, i))
    &&& (!b.nodes@[i].pool && a.nodes@[i].pool
        ==> pool_users(b.nodes@, b.nodes@.len()) < b.pool_capacity && oldest_waiter(b, i))
    &&& (!b.nodes@[i].holds && a.nodes@[i].holds
        ==> !b.nodes@[i].paused && scheduler::slot_spec(b, i)
            && a.nodes@[i].repo == b.nodes@[i].repo
            && a.nodes@[i].footprint == b.nodes@[i].footprint
            && a.nodes@[i].ticket.is_none()
            && a.nodes@[i].pool == b.nodes@[i].pool
            && a.nodes@[i].run == b.nodes@[i].run)
}

/// scheduler::advance: advance_post, E3.1 grant, E3.2 stutter, E4.1 reservation, E5.6 (v2.8).
#[verifier::opaque]
pub(super) open spec fn advanced_grants(b: World, a: World, i: int, r: bool) -> bool {
    &&& advance_post(b.nodes@[i], a.nodes@[i], b.next_ticket, a.next_ticket, r)
    &&& (advance_family_post::ready_unobstructed(b.nodes@[i])
        && scheduler::slot_spec(b, i) && oldest_waiter(b, i)
        ==> r && a.nodes@[i].holds && a.nodes@[i].ticket.is_none())
    &&& (advance_family_post::ready_unobstructed(b.nodes@[i]) && !r
        ==> a.nodes@[i] == b.nodes@[i])
    &&& (advance_family_post::ready_unobstructed(b.nodes@[i])
        ==> forall|older: int| 0 <= older < b.nodes@.len()
            && b.nodes@[older].phase is Ready
            && b.nodes@[older].repo == b.nodes@[i].repo
            && b.nodes@[older].ticket.is_some()
            && b.nodes@[older].ticket.unwrap() < b.nodes@[i].ticket.unwrap()
            && scheduler::slot_spec(b, older)
            && !scheduler::overlapped_by_older(b, older, b.nodes@[older].ticket.unwrap())
            ==> !r && a.nodes@[i] == b.nodes@[i])
    &&& (b.nodes@[i].holds && !b.nodes@[i].paused
        && current_item(b.nodes@[i]) == Some(Item::Do(EffectKind::PrepareWorktree))
        ==> a.nodes@[i].holds)
    &&& (b.nodes@[i].holds && current_item(b.nodes@[i]) == Some(Item::Await) ==> a.nodes@[i].holds)
}

/// REQUESTED from scheduler::advance (not exported yet).
/// Q1: first_in_line's K4c clause carried through the grant: a granted Ready waiter overlaps
///     no older same-repository Ready waiter.
/// Q2: the world-level granted branch of advance_family_post::slot_grant / slot_grant_value.
/// Q3: an Await item with no waiting incident does not move (advance_enabled is false).
#[verifier::opaque]
pub(super) open spec fn advanced_requested(b: World, a: World, i: int, r: bool) -> bool {
    &&& (advance_family_post::ready_unobstructed(b.nodes@[i]) && r
        ==> forall|j: int| 0 <= j < b.nodes@.len()
            && b.nodes@[j].phase is Ready
            && b.nodes@[j].repo == b.nodes@[i].repo
            && b.nodes@[j].ticket.is_some()
            && b.nodes@[j].ticket.unwrap() < b.nodes@[i].ticket.unwrap()
            ==> true)
    &&& (advance_family_post::ready_unobstructed(b.nodes@[i]) && r
        ==> a.nodes@[i].holds && a.nodes@[i].ticket.is_none()
            && a.nodes@[i].phase == resumed_phase(
                match b.nodes@[i].phase { Phase::Ready(x) => x, _ => Resume::Making })
            && a.nodes@[i].pos == 0 && a.nodes@[i].paused == b.nodes@[i].paused)
    &&& (!b.nodes@[i].waiting_incident && current_item(b.nodes@[i]) == Some(Item::Await) ==> !r)
}

/// scheduler::advance E11.3 (a)-(d) and its v2.7 framing, for node i.
#[verifier::opaque]
pub(super) open spec fn advanced_teardown(b: World, a: World, i: int, r: bool) -> bool {
    let n = b.nodes@[i];
    let m = a.nodes@[i];
    let g = n.phase is Stopping && !n.waiting_incident && n.effect.is_none() && n.question.is_none();
    &&& (g && (match current_item(n) {
            Some(Item::Teardown(k)) => advance_family_post::teardown_gone(n, k), _ => false })
        ==> r && m.pos == n.pos + 1 && m.phase == n.phase && m.effect.is_none()
            && m.holds == n.holds && m.run == n.run && m.job == n.job && m.pr_open == n.pr_open
            && m.worktree == n.worktree && m.next_seq == n.next_seq
            && m.pool == (n.pool && current_item(n) != Some(Item::Teardown(EffectKind::StopRun))))
    &&& (g && (match current_item(n) {
            Some(Item::Teardown(k)) => !advance_family_post::teardown_gone(n, k), _ => false })
        ==> r && m.effect.is_some() && m.effect.unwrap().seq == n.next_seq
            && current_item(n) == Some(Item::Teardown(m.effect.unwrap().kind))
            && !m.effect.unwrap().cancelling && m.next_seq == n.next_seq + 1
            && m.pos == n.pos && m.phase == n.phase && m.holds == n.holds
            && m.pool == n.pool && m.run == n.run && m.job == n.job && m.pr_open == n.pr_open
            && m.worktree == n.worktree)
    &&& (g && current_item(n) == Some(Item::Release)
        ==> r && !m.holds && m.pos == n.pos + 1 && m.phase == n.phase && m.effect.is_none()
            && m.pool == n.pool && m.run == n.run && m.job == n.job && m.pr_open == n.pr_open
            && m.worktree == n.worktree && m.next_seq == n.next_seq)
    &&& (g && current_item(n).is_none()
        ==> r && m.holds == n.holds && m.pos == 0
            && (match n.phase { Phase::Stopping(o) => m.phase == Phase::Closed(o), _ => true }))
    &&& (g && current_item(n).is_some()
        ==> m.run_failures == n.run_failures && m.run_incidents == n.run_incidents
            && m.fx_failures == n.fx_failures && m.fx_incidents == n.fx_incidents
            && m.question == n.question && m.credit == n.credit
            && m.generation == n.generation && m.ticket == n.ticket)
}

/// on_command(w, i, id, Command::Stop, out): existing frames, rows and E11.1.
#[verifier::opaque]
pub(super) open spec fn stopped(b: World, a: World, i: int, id: u64) -> bool {
    let n = b.nodes@[i];
    let m = a.nodes@[i];
    &&& 0 <= i < b.nodes@.len()
    &&& m.repo == n.repo && m.footprint == n.footprint
    &&& (settle_shape(n) ==> settle_shape(m))
    &&& a.nodes@ == b.nodes@.update(i, m)
    &&& a.next_ticket == b.next_ticket
    &&& b.next_question <= a.next_question
    &&& a.next_question as int <= b.next_question as int + 1
    &&& a.gates@ == b.gates@ && a.pool_capacity == b.pool_capacity
    &&& a.file_sets@ == b.file_sets@ && a.dependencies@ == b.dependencies@
    &&& (m.holds ==> n.holds) && (m.pool ==> n.pool)
    &&& (m.ticket == n.ticket || m.ticket.is_none())
    &&& (retirement::node_rules(b, n) ==> retirement::node_rules(a, m))
    &&& (id > n.last_cmd && !(n.phase is Closed || n.phase is Stopping)
        ==> m.phase == Phase::Stopping(Outcome::Stopped) && m.pos == 0
            && m.ticket.is_none() && m.question.is_none() && !m.waiting_incident
            && m.holds == n.holds && m.pool == n.pool && m.run == n.run && m.job == n.job
            && m.pr_open == n.pr_open && m.worktree == n.worktree
            && m.next_seq == n.next_seq && m.last_cmd == id
            && m.run_failures == n.run_failures && m.run_incidents == n.run_incidents
            && m.fx_failures == n.fx_failures && m.fx_incidents == n.fx_incidents
            && m.credit == n.credit && m.generation as int == n.generation as int + 1
            && (match n.effect {
                None => m.effect.is_none(),
                Some(e) => m.effect.is_some() && m.effect.unwrap().seq == e.seq
                    && m.effect.unwrap().kind == e.kind && m.effect.unwrap().cancelling,
            }))
}

/// on_ack's E11.2 premise for a teardown step: Confirmed, matching, not cancelled.
pub(super) open spec fn confirms_teardown(n: Node, seq: u64) -> bool {
    n.phase is Stopping && match n.effect {
        Some(e) => e.seq == seq && !e.cancelling && transition_safety::teardown_kind(e.kind)
            && current_item(n) == Some(Item::Teardown(e.kind)),
        None => false,
    }
}

/// on_ack(w, i, seq, Ack::Confirmed, out): existing frames, E11.2 R25 and its v2.5/v2.7 framing.
#[verifier::opaque]
pub(super) open spec fn acked(b: World, a: World, i: int, seq: u64) -> bool {
    let n = b.nodes@[i];
    let m = a.nodes@[i];
    &&& 0 <= i < b.nodes@.len()
    &&& m.repo == n.repo && m.footprint == n.footprint
    &&& (settle_shape(n) && stopping_effect_fits(n) ==> settle_shape(m))
    &&& a.nodes@.len() == b.nodes@.len()
    &&& forall|j: int| 0 <= j < b.nodes@.len() && j != i ==> #[trigger] a.nodes@[j] == b.nodes@[j]
    &&& m.credit == n.credit
    &&& a.next_ticket == b.next_ticket
    &&& a.gates@ == b.gates@ && a.pool_capacity == b.pool_capacity
    &&& a.file_sets@ == b.file_sets@ && a.dependencies@ == b.dependencies@
    &&& (confirms_teardown(n, seq)
        ==> m.effect.is_none() && m.pos == n.pos + 1 && m.phase == n.phase
            && m.holds == n.holds && m.next_seq == n.next_seq && m.question == n.question
            && m.run_failures == n.run_failures && m.run_incidents == n.run_incidents
            && m.fx_failures == 0 && m.fx_incidents == 0
            && m.run == (if n.effect.unwrap().kind == EffectKind::StopRun { None } else { n.run })
            && m.pool == (if n.effect.unwrap().kind == EffectKind::StopRun { false } else { n.pool })
            && m.job == (if n.effect.unwrap().kind == EffectKind::WithdrawJob { None } else { n.job })
            && m.pr_open == (if n.effect.unwrap().kind == EffectKind::ClosePr { false } else { n.pr_open })
            && m.worktree == (if n.effect.unwrap().kind == EffectKind::RemoveWorktree { false }
                else { n.worktree }))
    &&& (confirms_teardown(n, seq)
        ==> m.ticket == n.ticket && m.generation == n.generation && a.next_question == b.next_question)
    &&& (retirement::node_rules(b, n) && confirms_teardown(n, seq)
        ==> retirement::node_rules(a, m))
}

/// REQUESTED (E11 framing, not exported yet): a Confirmed teardown acknowledgement (E11.2 R25)
/// and the teardown advance steps (E11.3 (a) skip, (c) Release) keep `waiting_incident`.
#[verifier::opaque]
pub(super) open spec fn teardown_requested(b: World, a: World, i: int) -> bool {
    a.nodes@[i].waiting_incident == b.nodes@[i].waiting_incident
}

/// Summary of the harness's whole teardown of node 1 (proved in liveness_gate_teardown).
#[verifier::opaque]
pub(super) open spec fn torn_down(b: World, a: World) -> bool {
    &&& a.nodes@.len() == b.nodes@.len()
    &&& forall|j: int| 0 <= j < b.nodes@.len() && j != 1 ==> #[trigger] a.nodes@[j] == b.nodes@[j]
    &&& a.gates@ == b.gates@
    &&& a.file_sets@ == b.file_sets@
    &&& a.nodes@[1].phase is Closed
    &&& !a.nodes@[1].holds
    &&& (liveness_gate_domain::healthy(b) ==> liveness_gate_domain::healthy(a))
}
}
