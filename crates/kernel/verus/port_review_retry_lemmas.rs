//! #806: exact branch frames separate retry equations from executable queries.
use super::port_review_retry::*;
use super::*;

verus! {
/// Q54: stopping for review forwards the independently verified exact node update.
pub(super) proof fn rung_stop(n: Node, after: Node, step: ReviewStop, id: u64, next: u64)
    requires after == review_stopped(n, step), next == id,
    ensures rung_post(n, after, true, id, next, step),
{ reveal(rung_post); reveal(rung); }

/// State-table unfunded recheck: only the listed retry fields may change.
#[verifier::opaque]
pub(super) open spec fn recheck_question_frame(n: Node) -> ((RepoId, Footprint, u64, Option<u64>, Option<u64>, bool, bool, u64,), (u8, u8, bool, u8, u8, Option<EarlyEnd>, u8, u8,), (u8, u8, bool, bool, Option<HeadId>, bool, Option<HeadId>, Option<HeadId>,), (Option<HeadId>, Option<HeadId>, bool, bool, bool, bool, Credit, Priority,), (Option<ReviewStop>, bool, Option<u64>,),) { ((n.repo, n.footprint, n.next_seq, n.run, n.job, n.pool, n.paused, n.last_cmd,), (n.attempt, n.round, n.replanned, n.plan_retries, n.audit_rounds, n.early_end, n.run_failures, n.run_incidents,), (n.fx_failures, n.fx_incidents, n.waiting_incident, n.incident_needs_rob, n.proposed, n.adopted, n.head, n.checked,), (n.approved, n.merged, n.pr_open, n.had_pr, n.worktree, n.review_restarted, n.credit, n.priority,), (n.review_stop, n.planning_started, n.waiting_prerequisite,),) }

/// State-table unfunded recheck: scalar changes and the unchanged projection fold the exact result.
pub(super) proof fn recheck_question(n: Node, after: Node, target: Phase, id: u64, next: u64)
    requires (if n.rechecks < u8::MAX { (n.rechecks + 1) as u8 } else { u8::MAX }) > FREE_RECHECKS, !funded(n), next == (id + 1) as u64,
        recheck_question_frame(after) == recheck_question_frame(n),
        after.generation == ((n.generation + 1) as u64),
        after.pos == (0),
        after.ticket == (None),
        after.effect == (match n.effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None }),
        after.phase == (Phase::AwaitingRob),
        after.holds == (false),
        after.rechecks == (if n.rechecks < u8::MAX { (n.rechecks + 1) as u8 } else { u8::MAX }),
        after.question == (Some(Question { id, kind: QKind::OutOfCredit, cont: Cont::Ready(if target == Phase::Reviewing { Resume::Reviewing } else { Resume::Checking }), cost: 1, clarified: false })),
    ensures recheck_post(n, after, target, id, next),
{ reveal(recheck_question_frame); reveal(recheck_post); reveal(rechecked); }

/// #806 recheck: fold the two credit branches after the common phase change.
pub(super) proof fn recheck_resumed(n: Node, ready: Node, after: Node,
    target: Phase, id: u64, next: u64, free: bool)
    requires next == id,
        free == ((if n.rechecks < u8::MAX { (n.rechecks + 1) as u8 } else { u8::MAX }) <= FREE_RECHECKS),
        free || funded(n),
        ready == (Node {
            rechecks: if n.rechecks < u8::MAX { (n.rechecks + 1) as u8 } else { u8::MAX },
            holds: holding(target) && n.holds,
            credit: if free { n.credit } else { Credit { spent: (n.credit.spent + 1) as u64, ..n.credit } },
            ..n }),
        after == retirement::phased(ready, target),
    ensures recheck_post(n, after, target, id, next),
{ reveal(recheck_post); reveal(rechecked); }

/// State-table paid attempt: only the listed retry fields may change.
#[verifier::opaque]
pub(super) open spec fn rung_attempt_paid_frame(n: Node) -> ((RepoId, Footprint, u64, Option<u64>, Option<u64>, bool, bool, bool,), (u64, Option<Question>, bool, u8, u8, Option<EarlyEnd>, u8, u8,), (u8, u8, bool, bool, Option<HeadId>, bool, Option<HeadId>, Option<HeadId>,), (Option<HeadId>, Option<HeadId>, bool, bool, bool, u8, Priority, Option<ReviewStop>,), (bool, Option<u64>,),) { ((n.repo, n.footprint, n.next_seq, n.run, n.job, n.pool, n.holds, n.paused,), (n.last_cmd, n.question, n.replanned, n.plan_retries, n.audit_rounds, n.early_end, n.run_failures, n.run_incidents,), (n.fx_failures, n.fx_incidents, n.waiting_incident, n.incident_needs_rob, n.proposed, n.adopted, n.head, n.checked,), (n.approved, n.merged, n.pr_open, n.had_pr, n.worktree, n.rechecks, n.priority, n.review_stop,), (n.planning_started, n.waiting_prerequisite,),) }

/// State-table paid attempt: scalar changes and the unchanged projection fold the exact result.
pub(super) proof fn rung_attempt_paid(n: Node, after: Node, step: ReviewStop, id: u64, next: u64)
    requires n.attempt < 3, funded(n), next == id,
        rung_attempt_paid_frame(after) == rung_attempt_paid_frame(n),
        after.generation == ((n.generation + 1) as u64),
        after.pos == (0),
        after.ticket == (None),
        after.effect == (match n.effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None }),
        after.attempt == ((n.attempt + 1) as u8),
        after.review_restarted == (false),
        after.round == (0),
        after.phase == (Phase::Making),
        after.credit == (Credit { spent: (n.credit.spent + 1) as u64, ..n.credit }),
    ensures rung_post(n, after, false, id, next, step),
{ reveal(rung_attempt_paid_frame); reveal(rung_post); reveal(rung); }

/// State-table unfunded attempt: only the listed retry fields may change.
#[verifier::opaque]
pub(super) open spec fn rung_attempt_question_frame(n: Node) -> ((RepoId, Footprint, u64, Option<u64>, Option<u64>, bool, bool, u64,), (bool, u8, u8, Option<EarlyEnd>, u8, u8, u8, u8,), (bool, bool, Option<HeadId>, bool, Option<HeadId>, Option<HeadId>, Option<HeadId>, Option<HeadId>,), (bool, bool, bool, u8, Credit, Priority, Option<ReviewStop>, bool,), (Option<u64>,),) { ((n.repo, n.footprint, n.next_seq, n.run, n.job, n.pool, n.paused, n.last_cmd,), (n.replanned, n.plan_retries, n.audit_rounds, n.early_end, n.run_failures, n.run_incidents, n.fx_failures, n.fx_incidents,), (n.waiting_incident, n.incident_needs_rob, n.proposed, n.adopted, n.head, n.checked, n.approved, n.merged,), (n.pr_open, n.had_pr, n.worktree, n.rechecks, n.credit, n.priority, n.review_stop, n.planning_started,), (n.waiting_prerequisite,),) }

/// State-table unfunded attempt: scalar changes and the unchanged projection fold the exact result.
pub(super) proof fn rung_attempt_question(n: Node, after: Node, step: ReviewStop, id: u64, next: u64)
    requires n.attempt < 3, !funded(n), next == (id + 1) as u64,
        rung_attempt_question_frame(after) == rung_attempt_question_frame(n),
        after.generation == ((n.generation + 1) as u64),
        after.pos == (0),
        after.ticket == (None),
        after.effect == (match n.effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None }),
        after.attempt == ((n.attempt + 1) as u8),
        after.review_restarted == (false),
        after.round == (0),
        after.phase == (Phase::AwaitingRob),
        after.holds == (false),
        after.question == (Some(Question { id, kind: QKind::OutOfCredit, cont: Cont::Ready(Resume::Making), cost: 1, clarified: false })),
    ensures rung_post(n, after, false, id, next, step),
{ reveal(rung_attempt_question_frame); reveal(rung_post); reveal(rung); }

/// State-table paid replan: only the listed retry fields may change.
#[verifier::opaque]
pub(super) open spec fn rung_replan_paid_frame(n: Node) -> ((RepoId, Footprint, u64, Option<u64>, Option<u64>, bool, bool, u64,), (Option<Question>, u8, u8, u8, Option<EarlyEnd>, u8, u8, u8,), (u8, bool, bool, Option<HeadId>, bool, Option<HeadId>, Option<HeadId>, Option<HeadId>,), (Option<HeadId>, bool, bool, bool, u8, bool, Priority, Option<ReviewStop>,), (bool, Option<u64>,),) { ((n.repo, n.footprint, n.next_seq, n.run, n.job, n.pool, n.paused, n.last_cmd,), (n.question, n.attempt, n.round, n.audit_rounds, n.early_end, n.run_failures, n.run_incidents, n.fx_failures,), (n.fx_incidents, n.waiting_incident, n.incident_needs_rob, n.proposed, n.adopted, n.head, n.checked, n.approved,), (n.merged, n.pr_open, n.had_pr, n.worktree, n.rechecks, n.review_restarted, n.priority, n.review_stop,), (n.planning_started, n.waiting_prerequisite,),) }

/// State-table paid replan: scalar changes and the unchanged projection fold the exact result.
pub(super) proof fn rung_replan_paid(n: Node, after: Node, step: ReviewStop, id: u64, next: u64)
    requires n.attempt >= 3, !n.replanned, funded(n), next == id,
        rung_replan_paid_frame(after) == rung_replan_paid_frame(n),
        after.generation == ((n.generation + 1) as u64),
        after.pos == (0),
        after.ticket == (None),
        after.effect == (match n.effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None }),
        after.phase == (Phase::Planning { must_split: false }),
        after.holds == (false),
        after.replanned == (true),
        after.plan_retries == (0),
        after.credit == (Credit { spent: (n.credit.spent + 1) as u64, ..n.credit }),
    ensures rung_post(n, after, false, id, next, step),
{ reveal(rung_replan_paid_frame); reveal(rung_post); reveal(rung); }

/// State-table unfunded replan: only the listed retry fields may change.
#[verifier::opaque]
pub(super) open spec fn rung_replan_question_frame(n: Node) -> ((RepoId, Footprint, u64, Option<u64>, Option<u64>, bool, bool, u64,), (u8, u8, u8, u8, Option<EarlyEnd>, u8, u8, u8,), (u8, bool, bool, Option<HeadId>, bool, Option<HeadId>, Option<HeadId>, Option<HeadId>,), (Option<HeadId>, bool, bool, bool, u8, bool, Credit, Priority,), (Option<ReviewStop>, bool, Option<u64>,),) { ((n.repo, n.footprint, n.next_seq, n.run, n.job, n.pool, n.paused, n.last_cmd,), (n.attempt, n.round, n.plan_retries, n.audit_rounds, n.early_end, n.run_failures, n.run_incidents, n.fx_failures,), (n.fx_incidents, n.waiting_incident, n.incident_needs_rob, n.proposed, n.adopted, n.head, n.checked, n.approved,), (n.merged, n.pr_open, n.had_pr, n.worktree, n.rechecks, n.review_restarted, n.credit, n.priority,), (n.review_stop, n.planning_started, n.waiting_prerequisite,),) }

/// State-table unfunded replan: scalar changes and the unchanged projection fold the exact result.
pub(super) proof fn rung_replan_question(n: Node, after: Node, step: ReviewStop, id: u64, next: u64)
    requires n.attempt >= 3, !n.replanned, !funded(n), next == (id + 1) as u64,
        rung_replan_question_frame(after) == rung_replan_question_frame(n),
        after.generation == ((n.generation + 1) as u64),
        after.pos == (0),
        after.ticket == (None),
        after.effect == (match n.effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None }),
        after.phase == (Phase::AwaitingRob),
        after.holds == (false),
        after.replanned == (true),
        after.question == (Some(Question { id, kind: QKind::OutOfCredit, cont: Cont::Planning { must_split: false }, cost: 1, clarified: false })),
    ensures rung_post(n, after, false, id, next, step),
{ reveal(rung_replan_question_frame); reveal(rung_post); reveal(rung); }

/// State-table exhausted ladder: only the listed retry fields may change.
#[verifier::opaque]
pub(super) open spec fn rung_question_frame(n: Node) -> ((RepoId, Footprint, u64, Option<u64>, Option<u64>, bool, bool, u64,), (u8, u8, bool, u8, u8, Option<EarlyEnd>, u8, u8,), (u8, u8, bool, bool, Option<HeadId>, bool, Option<HeadId>, Option<HeadId>,), (Option<HeadId>, Option<HeadId>, bool, bool, bool, u8, bool, Credit,), (Priority, Option<ReviewStop>, bool, Option<u64>,),) { ((n.repo, n.footprint, n.next_seq, n.run, n.job, n.pool, n.paused, n.last_cmd,), (n.attempt, n.round, n.replanned, n.plan_retries, n.audit_rounds, n.early_end, n.run_failures, n.run_incidents,), (n.fx_failures, n.fx_incidents, n.waiting_incident, n.incident_needs_rob, n.proposed, n.adopted, n.head, n.checked,), (n.approved, n.merged, n.pr_open, n.had_pr, n.worktree, n.rechecks, n.review_restarted, n.credit,), (n.priority, n.review_stop, n.planning_started, n.waiting_prerequisite,),) }

/// State-table exhausted ladder: scalar changes and the unchanged projection fold the exact result.
pub(super) proof fn rung_question(n: Node, after: Node, step: ReviewStop, id: u64, next: u64)
    requires n.attempt >= 3, n.replanned, next == (id + 1) as u64,
        rung_question_frame(after) == rung_question_frame(n),
        after.generation == ((n.generation + 1) as u64),
        after.pos == (0),
        after.ticket == (None),
        after.effect == (match n.effect { Some(e) => Some(InFlight { cancelling: true, ..e }), None => None }),
        after.phase == (Phase::AwaitingRob),
        after.holds == (false),
        after.question == (Some(Question { id, kind: QKind::CantMeet, cont: Cont::Ready(Resume::Making), cost: 1, clarified: false })),
    ensures rung_post(n, after, false, id, next, step),
{ reveal(rung_question_frame); reveal(rung_post); reveal(rung); }

/// #771: the exact recheck preserves agenda and resource shape.
pub(super) proof fn recheck_shape(n: Node, after: Node, target: Phase, id: u64, next: u64)
    requires recheck_post(n, after, target, id, next), !(target is Closed),
    ensures settle_shape(n) ==> settle_shape(after),
{
    reveal(recheck_post); reveal(rechecked);
}

/// #771: each exact retry branch preserves agenda and resource shape.
pub(super) proof fn rung_shape(n: Node, after: Node, fail_fast: bool, id: u64, next: u64, step: ReviewStop)
    requires rung_post(n, after, fail_fast, id, next, step),
    ensures settle_shape(n) ==> settle_shape(after),
{
    reveal(rung_post); reveal(rung); reveal(retirement::stopped);
}

/// #805 rebase progress row: the exact recheck result advances or awaits Rob.
pub(super) proof fn recheck_progress(n: Node, after: Node, target: Phase, id: u64, next: u64)
    requires recheck_post(n, after, target, id, next),
    ensures ordinary_await(n) && n.phase == Phase::Rebasing && target == Phase::Checking
        ==> distance(after) < distance(n),
{
    reveal(recheck_post); reveal(rechecked);
    reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank);
}
}
