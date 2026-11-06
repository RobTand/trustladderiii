//! Q54 / #440 / #806: one local lemma for every stop-for-review diversion.
use super::*;

verus! {
/// Q54: all work-retry diversions preserve the ledger and ladder and ask nothing.
/// The exact update comes from the real stop_for_review contract, for any reason.
pub(super) proof fn diversion(n: Node, after: Node, step: ReviewStop)
    requires n.generation < u64::MAX, !(n.phase is Stopping || n.phase is Closed),
        after == retirement::stopped(Node { review_stop: Some(step), ..n }, Outcome::Stopped),
    ensures after.phase == Phase::Stopping(Outcome::Stopped), after.review_stop == Some(step),
        after.credit == n.credit, after.question.is_none(),
        after.attempt == n.attempt, after.replanned == n.replanned,
        after.proposed == n.proposed, after.adopted == n.adopted,
        after.next_seq == n.next_seq, after.generation as int == n.generation as int + 1,
{
    reveal(retirement::stopped);
}

/// Q54 / #806: the shared review-stop update satisfies every production node rule.
/// This is a local frame proof; it does not unroll a world or the checker loops.
pub(super) proof fn preserves_node(w: World, n: Node, step: ReviewStop)
    requires retirement::node_rules(w, n), node_headroom(n),
        !(n.phase is Stopping || n.phase is Closed),
    ensures retirement::node_rules(w,
        retirement::stopped(Node { review_stop: Some(step), ..n }, Outcome::Stopped)),
{
    reveal(retirement::node_rules);
    reveal(retirement::stopped);
}

/// Q54 reply rows: real end_operation precedes the retry diversion. Resetting
/// operation ownership and failure counters preserves the final stopping rules.
pub(super) proof fn preserves_completed_node(w: World, n: Node, after: Node, step: ReviewStop)
    requires retirement::node_rules(w, n), node_headroom(n),
        !(n.phase is Stopping || n.phase is Closed),
        after == retirement::stopped(Node { run: None, job: None, pool: false,
            run_failures: 0, run_incidents: 0, review_stop: Some(step),
            plan_retries: after.plan_retries,
            effect: match n.effect {
                Some(e) => Some(InFlight { cancelling: e.cancelling || matches!(e.kind,
                    EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }),
                None => None,
            }, ..n }, Outcome::Stopped),
    ensures retirement::node_rules(w, after), after.credit == n.credit,
        after.phase == Phase::Stopping(Outcome::Stopped), after.question.is_none(),
        after.review_stop == Some(step), after.attempt == n.attempt, after.replanned == n.replanned,
        after.proposed == n.proposed, after.adopted == n.adopted, after.next_seq == n.next_seq,
{
    reveal(retirement::node_rules);
    reveal(retirement::stopped);
}
}
