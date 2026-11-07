//! Failed-run branch facts, separate from mutable retry bookkeeping.
use super::*;

verus! {
/// Q30: abandoning an operation releases ownership and cancels its pending start.
#[verifier::inline]
pub(super) open spec fn abandoned(n: Node) -> Node {
    Node { run: None, job: None, pool: false,
        effect: match n.effect { Some(e) => Some(InFlight { cancelling: e.cancelling
            || matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }), None => None }, ..n }
}

/// Counting a run incident changes only its saturated incident counter.
pub(super) open spec fn incident_count_frame(b: Node, a: Node) -> bool {
    a.phase == b.phase
        && a.pos == b.pos
        && a.repo == b.repo
        && a.footprint == b.footprint
        && a.generation == b.generation
        && a.next_seq == b.next_seq
        && a.effect == b.effect
        && a.run == b.run
        && a.job == b.job
        && a.pool == b.pool
        && a.ticket == b.ticket
        && a.holds == b.holds
        && a.paused == b.paused
        && a.last_cmd == b.last_cmd
        && a.question == b.question
        && a.attempt == b.attempt
        && a.round == b.round
        && a.replanned == b.replanned
        && a.plan_retries == b.plan_retries
        && a.audit_rounds == b.audit_rounds
        && a.early_end == b.early_end
        && a.run_failures == b.run_failures
        && a.fx_failures == b.fx_failures
        && a.fx_incidents == b.fx_incidents
        && a.waiting_incident == b.waiting_incident
        && a.incident_needs_rob == b.incident_needs_rob
        && a.proposed == b.proposed
        && a.adopted == b.adopted
        && a.head == b.head
        && a.checked == b.checked
        && a.approved == b.approved
        && a.merged == b.merged
        && a.pr_open == b.pr_open
        && a.had_pr == b.had_pr
        && a.worktree == b.worktree
        && a.rechecks == b.rechecks
        && a.review_restarted == b.review_restarted
        && a.credit == b.credit
        && a.priority == b.priority
        && a.review_stop == b.review_stop
        && a.planning_started == b.planning_started
        && a.waiting_prerequisite == b.waiting_prerequisite
}

#[verifier::opaque]
pub(super) open spec fn incident_count_update(b: Node, a: Node) -> bool {
    incident_count_frame(b, a)
        && a.run_incidents == if b.run_incidents < 2 { (b.run_incidents + 1) as u8 } else { 2u8 }
}

pub(super) proof fn write_incident_count(b: Node, a: Node)
    requires incident_count_frame(b, a),
        a.run_incidents == if b.run_incidents < 2 { (b.run_incidents + 1) as u8 } else { 2u8 },
    ensures incident_count_update(b, a),
{
    reveal(incident_count_update);
}

/// The failed-run consumer needs counter and field facts, not a whole-node equation.
pub(super) proof fn read_incident_count(b: Node, a: Node)
    requires incident_count_update(b, a),
    ensures incident_count_frame(b, a),
        a.run_incidents == if b.run_incidents < 2 { (b.run_incidents + 1) as u8 } else { 2u8 },
{
    reveal(incident_count_update);
}

/// Q50: hide the complete failed-run update while proving each branch.
/// Must equal `port_t860_contracts::failed_node` for `run_failures <= 1`.
/// `failed_result` proves the connection through `failed_post`.
#[verifier::opaque]
pub(super) open spec fn failed_run(n: Node) -> Node {
    let m = Node { pos: if n.phase == Phase::Making { 1u8 } else { 0u8 },
        generation: (n.generation + 1) as u64,
        run_failures: (n.run_failures + 1) as u8, ..abandoned(n) };
    if m.run_failures >= 2 {
        let incident = Node { run_failures: 0,
            run_incidents: if n.run_incidents < 2 { (n.run_incidents + 1) as u8 } else { 2u8 }, ..m };
        retirement::incident(incident, incident.run_incidents >= 2)
    } else { m }
}

/// Q50 failed-run row: the first failure restarts the same operation once.
pub(super) proof fn restarted(before: Node, after: Node)
    requires before.run_failures == 0,
        after == (Node {
            pos: if before.phase == Phase::Making { 1u8 } else { 0u8 },
            generation: (before.generation + 1) as u64,
            run_failures: 1, ..abandoned(before)
        }),
    ensures after == failed_run(before),
{
    reveal(abandoned);
    reveal(failed_run);
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
    ensures retirement::incident(ready, needs_rob) == failed_run(before),
{
    reveal(abandoned);
    reveal(failed_run);
}

/// Q50: package the exact result and notice frame without exposing either to callers.
pub(super) proof fn failed_result(before: Node, after: Node, bo: Outbox, ao: Outbox)
    requires before.run_failures <= 1, after == failed_run(before),
        retirement::notices_only(bo) ==> retirement::notices_only(ao),
    ensures port_t860_contracts::failed_post(before, after, bo, ao),
{
    reveal(port_t860_contracts::failed_post);
    reveal(port_t860_contracts::failed_node);
    reveal(failed_run);
}
}
