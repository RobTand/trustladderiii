//! #806: node-only contracts for the five independently framed answers.
use super::port_answers_model::*;
use super::*;

verus! {
/// #806: no question, stale question, unrelated phase or unsupported offer stutters.
#[verifier::spinoff_prover]
pub(super) proof fn post_unchanged(n: Node, id: u64, answer: Answer, next: u64)
    requires answers::answer_supported(answer),
        match n.question {
            None => true,
            Some(q) => q.id != id || match n.phase {
                Phase::Stopping(_) => answer != Answer::Retry && answer != Answer::MarkHandled,
                Phase::AwaitingRob => answer != Answer::Unclear
                    && (!offered(q.kind, answer) || matches!(answer, Answer::Retry | Answer::MarkHandled)),
                _ => true,
            },
        },
    ensures answers::answer_post(n, n, id, answer, next, next),
{
    reveal(answers::answer_post); reveal(answered);
}

/// #806: Retry clears only a current stopping question.
#[verifier::spinoff_prover]
pub(super) proof fn post_retry(n: Node, id: u64, next: u64)
    requires n.phase is Stopping, n.question.is_some(), n.question.unwrap().id == id,
    ensures answers::answer_post(n, Node { question: None, ..n }, id, Answer::Retry, next, next),
{
    reveal(answers::answer_post); reveal(answered);
}

/// #806: one clarification changes only the question and its allocator.
#[verifier::spinoff_prover]
pub(super) proof fn post_unclear(n: Node, id: u64, next: u64)
    requires n.phase == Phase::AwaitingRob, n.question.is_some(), n.question.unwrap().id == id,
    ensures answers::answer_post(n,
        if n.question.unwrap().clarified { n } else {
            Node { question: Some(Question { id: next, clarified: true, ..n.question.unwrap() }), ..n }
        }, id, Answer::Unclear, next, if n.question.unwrap().clarified { next } else { (next + 1) as u64 }),
{
    reveal(answers::answer_post); reveal(answered);
}

/// #806: Stop and ChangeRequest delegate to the existing exact stopping frame.
#[verifier::spinoff_prover]
pub(super) proof fn post_stopping(n: Node, id: u64, answer: Answer, next: u64)
    requires n.phase == Phase::AwaitingRob, n.question.is_some(), n.question.unwrap().id == id,
        matches!(answer, Answer::Stop | Answer::ChangeRequest), offered(n.question.unwrap().kind, answer),
    ensures answers::answer_post(n,
        retirement::stopped(n, if answer == Answer::Stop { Outcome::Stopped } else { Outcome::Replaced }),
        id, answer, next, next),
{
    reveal(answers::answer_post); reveal(answered);
}

/// #805 / #806: cleanup-answer eligibility gives a teardown item, without a world in the handler.
#[verifier::spinoff_prover]
pub(super) proof fn cleanup_input(w: World, n: Node, id: u64, answer: Answer)
    ensures node_checked(w, n) && answer_progress_input(n, id, answer) && n.phase is Stopping
        ==> answer == Answer::MarkHandled && n.effect.is_none() && !n.waiting_incident
            && matches!(current_item(Node { question: None, ..n }), Some(Item::Teardown(_))),
{}

/// #805 / #806: clearing a stopping question removes four units and preserves cleanup shape.
#[verifier::spinoff_prover]
pub(super) proof fn stopping_question(n: Node)
    requires n.phase is Stopping, n.question.is_some(),
    ensures settle_shape(Node { question: None, ..n }) == settle_shape(n),
        n.effect.is_none() && !n.waiting_incident
            ==> distance(Node { question: None, ..n }) + 4 == distance(n),
{ reveal(distance); }
/// #806: reconstruct the private cleanup model from W3's literal MarkHandled field contract.
#[verifier::spinoff_prover]
pub(super) proof fn handled_result(n: Node, after: Node)
    requires n.phase is Stopping,
        after.phase == n.phase,
        after.repo == n.repo,
        after.footprint == n.footprint,
        after.generation == n.generation,
        after.next_seq == n.next_seq,
        after.effect == n.effect,
        after.ticket == n.ticket,
        after.holds == n.holds,
        after.paused == n.paused,
        after.last_cmd == n.last_cmd,
        after.question == n.question,
        after.attempt == n.attempt,
        after.round == n.round,
        after.replanned == n.replanned,
        after.plan_retries == n.plan_retries,
        after.audit_rounds == n.audit_rounds,
        after.early_end == n.early_end,
        after.run_failures == n.run_failures,
        after.run_incidents == n.run_incidents,
        after.fx_failures == n.fx_failures,
        after.fx_incidents == n.fx_incidents,
        after.waiting_incident == n.waiting_incident,
        after.incident_needs_rob == n.incident_needs_rob,
        after.proposed == n.proposed,
        after.adopted == n.adopted,
        after.head == n.head,
        after.checked == n.checked,
        after.approved == n.approved,
        after.merged == n.merged,
        after.had_pr == n.had_pr,
        after.rechecks == n.rechecks,
        after.review_restarted == n.review_restarted,
        after.credit == n.credit,
        after.priority == n.priority,
        after.review_stop == n.review_stop,
        after.planning_started == n.planning_started,
        after.waiting_prerequisite == n.waiting_prerequisite,
        after.pos == if matches!(current_item(n), Some(Item::Teardown(_))) { (n.pos + 1) as u8 } else { n.pos },
        after.run == if matches!(current_item(n), Some(Item::Teardown(EffectKind::StopRun))) { None } else { n.run },
        after.pool == if matches!(current_item(n), Some(Item::Teardown(EffectKind::StopRun))) { false } else { n.pool },
        after.job == if matches!(current_item(n), Some(Item::Teardown(EffectKind::WithdrawJob))) { None } else { n.job },
        after.pr_open == if matches!(current_item(n), Some(Item::Teardown(EffectKind::ClosePr))) { false } else { n.pr_open },
        after.worktree == if matches!(current_item(n), Some(Item::Teardown(EffectKind::RemoveWorktree))) { false } else { n.worktree },
    ensures after == handled(n),
{ reveal(handled); }

/// #806: MarkHandled clears the named question before advancing the teardown resource.
#[verifier::spinoff_prover]
pub(super) proof fn post_handled(n: Node, id: u64, next: u64)
    requires n.phase is Stopping, n.question.is_some(), n.question.unwrap().id == id,
    ensures answers::answer_post(n, handled(Node { question: None, ..n }),
        id, Answer::MarkHandled, next, next),
{ reveal(answers::answer_post); reveal(answered); }

/// #806: keep the whole cleanup result opaque inside the World-level branch.
#[verifier::opaque]
pub(super) open spec fn cleanup_post(before: Node, after: Node) -> bool {
    after == handled(Node { question: None, ..before })
}

/// #806: seal the real MarkHandled result before returning to the branch handler.
#[verifier::spinoff_prover]
pub(super) proof fn seal_cleanup(before: Node, after: Node)
    requires after == handled(Node { question: None, ..before }),
    ensures cleanup_post(before, after),
{ reveal(cleanup_post); }

/// #806: reconstruct the Answer postcondition without exposing cleanup equality to its caller.
#[verifier::spinoff_prover]
pub(super) proof fn post_cleanup(before: Node, after: Node, id: u64, next: u64)
    requires before.phase is Stopping, before.question.is_some(), before.question.unwrap().id == id,
        cleanup_post(before, after),
    ensures answers::answer_post(before, after, id, Answer::MarkHandled, next, next),
{
    reveal(cleanup_post);
    post_handled(before, id, next);
}

}
