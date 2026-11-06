//! Concept-named field-group contracts for node updates. Spike C1.
use super::*;

verus! {
pub(super) open spec fn operation(n: Node) -> (Option<u64>, Option<u64>, bool, Option<InFlight>) {
    (n.run, n.job, n.pool, n.effect)
}

pub(super) open spec fn workflow(n: Node) -> (Phase, u8, u64, Option<u64>, bool) {
    (n.phase, n.pos, n.generation, n.ticket, n.holds)
}

pub(super) open spec fn failure_ladder(n: Node) -> (u8, u8, u8, u8) {
    (n.run_failures, n.run_incidents, n.fx_failures, n.fx_incidents)
}

pub(super) open spec fn incident(n: Node) -> (bool, bool) {
    (n.waiting_incident, n.incident_needs_rob)
}

pub(super) open spec fn pr_facts(n: Node) -> (Option<HeadId>, Option<HeadId>, Option<HeadId>, Option<HeadId>, Option<HeadId>, bool, bool, bool, bool) {
    (n.proposed, n.head, n.checked, n.approved, n.merged, n.adopted, n.pr_open, n.had_pr, n.worktree)
}

pub(super) open spec fn attempt_ladder(n: Node) -> (u8, u8, u8, u8, u8, bool, bool, bool, Option<EarlyEnd>, Option<ReviewStop>) {
    (n.attempt, n.round, n.plan_retries, n.audit_rounds, n.rechecks, n.replanned, n.review_restarted, n.planning_started, n.early_end, n.review_stop)
}

pub(super) open spec fn identity(n: Node) -> (RepoId, Footprint, u64, bool, u64, Option<Question>, Priority, Option<u64>) {
    (n.repo, n.footprint, n.next_seq, n.paused, n.last_cmd, n.question, n.priority, n.waiting_prerequisite)
}

pub(super) open spec fn ledger(n: Node) -> Credit { n.credit }

pub(super) open spec fn incident_resume(p: Phase) -> Phase {
    match p {
        Phase::Making => Phase::Ready(Resume::Making),
        Phase::Checking => Phase::Ready(Resume::Checking),
        Phase::Reviewing => Phase::Ready(Resume::Reviewing),
        Phase::Publishing | Phase::Queued => Phase::Ready(Resume::Publishing),
        Phase::Rebasing => Phase::Ready(Resume::Rebasing),
        _ => p,
    }
}

pub(super) open spec fn failed_operation(b: Node) -> (Option<u64>, Option<u64>, bool, Option<InFlight>) {
    (None, None, false, match b.effect {
        Some(e) => Some(InFlight { cancelling: e.cancelling
            || (b.run_failures != 0 && holding(b.phase))
            || matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }),
        None => None,
    })
}

pub(super) open spec fn failed_workflow(b: Node) -> (Phase, u8, u64, Option<u64>, bool) {
    let resumed = b.run_failures != 0 && holding(b.phase);
    (if resumed { incident_resume(b.phase) } else { b.phase },
        if resumed { 0u8 } else if b.phase == Phase::Making { 1u8 } else { 0u8 },
        (b.generation + 1 + if resumed { 1int } else { 0int }) as u64,
        if b.run_failures != 0 { None } else { b.ticket },
        !resumed && b.holds)
}

pub(super) open spec fn failed_ladder(b: Node) -> (u8, u8, u8, u8) {
    (if b.run_failures == 0 { 1u8 } else { 0u8 },
        if b.run_failures == 0 { b.run_incidents }
        else if b.run_incidents < 2 { (b.run_incidents + 1) as u8 } else { 2u8 },
        b.fx_failures, b.fx_incidents)
}

pub(super) open spec fn failed_incident(b: Node) -> (bool, bool) {
    (b.run_failures != 0 || b.waiting_incident,
        if b.run_failures != 0 { b.run_incidents >= 1 } else { b.incident_needs_rob })
}

/// A failed run changes only operation, workflow, failure and incident views.
#[verifier::opaque]
pub(super) open spec fn failure_frame(b: Node, a: Node) -> bool {
    pr_facts(a) == pr_facts(b) && attempt_ladder(a) == attempt_ladder(b)
        && ledger(a) == ledger(b) && identity(a) == identity(b)
}

#[verifier::opaque]
pub(super) open spec fn counted_failure(b: Node, a: Node, bo: Outbox, ao: Outbox) -> bool {
    operation(a) == failed_operation(b) && workflow(a) == failed_workflow(b)
        && failure_ladder(a) == failed_ladder(b) && incident(a) == failed_incident(b)
        && failure_frame(b, a)
        && (retirement::notices_only(bo) ==> retirement::notices_only(ao))
}

/// Package field facts without revealing the opaque update in the handler.
pub(super) proof fn count_failure_result(b: Node, a: Node, bo: Outbox, ao: Outbox)
    requires a.run == failed_operation(b).0,
        a.job == failed_operation(b).1,
        a.pool == failed_operation(b).2,
        a.effect == failed_operation(b).3,
        a.phase == failed_workflow(b).0,
        a.pos == failed_workflow(b).1,
        a.generation == failed_workflow(b).2,
        a.ticket == failed_workflow(b).3,
        a.holds == failed_workflow(b).4,
        a.run_failures == failed_ladder(b).0,
        a.run_incidents == failed_ladder(b).1,
        a.fx_failures == failed_ladder(b).2,
        a.fx_incidents == failed_ladder(b).3,
        a.waiting_incident == failed_incident(b).0,
        a.incident_needs_rob == failed_incident(b).1,
        a.proposed == b.proposed,
        a.head == b.head,
        a.checked == b.checked,
        a.approved == b.approved,
        a.merged == b.merged,
        a.adopted == b.adopted,
        a.pr_open == b.pr_open,
        a.had_pr == b.had_pr,
        a.worktree == b.worktree,
        a.attempt == b.attempt,
        a.round == b.round,
        a.plan_retries == b.plan_retries,
        a.audit_rounds == b.audit_rounds,
        a.rechecks == b.rechecks,
        a.replanned == b.replanned,
        a.review_restarted == b.review_restarted,
        a.planning_started == b.planning_started,
        a.early_end == b.early_end,
        a.review_stop == b.review_stop,
        a.repo == b.repo,
        a.footprint == b.footprint,
        a.next_seq == b.next_seq,
        a.paused == b.paused,
        a.last_cmd == b.last_cmd,
        a.question == b.question,
        a.priority == b.priority,
        a.waiting_prerequisite == b.waiting_prerequisite,
        a.credit == b.credit,
        retirement::notices_only(bo) ==> retirement::notices_only(ao),
    ensures counted_failure(b, a, bo, ao),
{
    reveal(counted_failure);
    reveal(failure_frame);
}

/// The new views retain every field of the original #860 contract.
pub(super) proof fn counted_failure_legacy(b: Node, a: Node, bo: Outbox, ao: Outbox)
    requires counted_failure(b, a, bo, ao), b.run_failures <= 1,
        b.run_incidents <= 2, b.generation as int + 2 <= u64::MAX as int,
    ensures port_t860_contracts::failed_post(b, a, bo, ao),
{
    reveal(counted_failure);
    reveal(failure_frame);
    reveal(port_t860_contracts::failed_post);
    reveal(port_t860_contracts::failed_node);
    reveal(retirement::incident);
}

pub(super) open spec fn raised_incident(b: Node, a: Node, needs_rob: bool) -> bool {
    a.run == b.run && a.job == b.job && a.pool == b.pool
    && a.effect == match b.effect {
        Some(e) => Some(InFlight { cancelling: e.cancelling || holding(b.phase), ..e }),
        None => None,
    }
    && a.phase == incident_resume(b.phase)
    && a.pos == (if holding(b.phase) { 0u8 } else { b.pos })
    && a.generation == (b.generation + if holding(b.phase) { 1int } else { 0int }) as u64
    && a.ticket.is_none() && a.holds == (!holding(b.phase) && b.holds)
    && a.run_failures == b.run_failures && a.run_incidents == b.run_incidents
    && a.fx_failures == b.fx_failures && a.fx_incidents == b.fx_incidents
    && a.waiting_incident && a.incident_needs_rob == needs_rob
    && a.proposed == b.proposed && a.head == b.head && a.checked == b.checked
    && a.approved == b.approved && a.merged == b.merged && a.adopted == b.adopted
    && a.pr_open == b.pr_open && a.had_pr == b.had_pr && a.worktree == b.worktree
    && a.attempt == b.attempt && a.round == b.round && a.plan_retries == b.plan_retries
    && a.audit_rounds == b.audit_rounds && a.rechecks == b.rechecks && a.replanned == b.replanned
    && a.review_restarted == b.review_restarted && a.planning_started == b.planning_started
    && a.early_end == b.early_end && a.review_stop == b.review_stop
    && a.credit == b.credit && a.repo == b.repo && a.footprint == b.footprint
    && a.next_seq == b.next_seq && a.paused == b.paused && a.last_cmd == b.last_cmd
    && a.question == b.question && a.priority == b.priority && a.waiting_prerequisite == b.waiting_prerequisite
}

/// Project an existing exact incident result once, outside its consumers.
pub(super) proof fn incident_views(b: Node, a: Node, needs_rob: bool)
    requires a == retirement::incident(b, needs_rob),
    ensures raised_incident(b, a, needs_rob),
{
    reveal(retirement::incident);
}


pub(super) open spec fn abandoned_fields(b: Node, a: Node) -> bool {
    a.run.is_none() && a.job.is_none() && !a.pool
    && a.effect == match b.effect {
        Some(e) => Some(InFlight { cancelling: e.cancelling
            || matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }),
        None => None,
    }
    && a.phase == b.phase && a.pos == b.pos && a.generation == b.generation
    && a.ticket == b.ticket && a.holds == b.holds
    && a.run_failures == b.run_failures && a.run_incidents == b.run_incidents
    && a.fx_failures == b.fx_failures && a.fx_incidents == b.fx_incidents
    && a.waiting_incident == b.waiting_incident && a.incident_needs_rob == b.incident_needs_rob
    && a.proposed == b.proposed && a.head == b.head && a.checked == b.checked
    && a.approved == b.approved && a.merged == b.merged && a.adopted == b.adopted
    && a.pr_open == b.pr_open && a.had_pr == b.had_pr && a.worktree == b.worktree
    && a.attempt == b.attempt && a.round == b.round && a.plan_retries == b.plan_retries
    && a.audit_rounds == b.audit_rounds && a.rechecks == b.rechecks && a.replanned == b.replanned
    && a.review_restarted == b.review_restarted && a.planning_started == b.planning_started
    && a.early_end == b.early_end && a.review_stop == b.review_stop
    && a.credit == b.credit && a.repo == b.repo && a.footprint == b.footprint
    && a.next_seq == b.next_seq && a.paused == b.paused && a.last_cmd == b.last_cmd
    && a.question == b.question && a.priority == b.priority && a.waiting_prerequisite == b.waiting_prerequisite
}

#[verifier::opaque]
pub(super) open spec fn abandoned_operation(b: Node, a: Node) -> bool {
    abandoned_fields(b, a)
}

pub(super) proof fn abandon_result(b: Node, a: Node)
    requires abandoned_fields(b, a),
    ensures abandoned_operation(b, a),
{ reveal(abandoned_operation); }

pub(super) proof fn abandon_legacy(b: Node, a: Node)
    requires abandoned_operation(b, a),
    ensures a == (Node { run: None, job: None, pool: false,
        effect: match b.effect {
            Some(e) => Some(InFlight { cancelling: e.cancelling
                || matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }),
            None => None,
        }, ..b }),
{ reveal(abandoned_operation); }

}
