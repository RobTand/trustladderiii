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
    requires operation(a) == failed_operation(b), workflow(a) == failed_workflow(b),
        failure_ladder(a) == failed_ladder(b), incident(a) == failed_incident(b),
        pr_facts(a) == pr_facts(b), attempt_ladder(a) == attempt_ladder(b),
        ledger(a) == ledger(b), identity(a) == identity(b),
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
}
