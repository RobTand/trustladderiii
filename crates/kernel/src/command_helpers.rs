//! Executable command safety contracts, split to bound each required query.
use super::*;

kernel_fn! {
/// Question answer rows / #771: cleanup, continuation and funding permissions.
pub(super) fn allowed(kind: QKind, answer: Answer) -> (result: bool) [
    ensures result == match (kind, answer) {
        (QKind::CleanupFailed, Answer::Retry | Answer::MarkHandled) => true,
        (QKind::CleanupFailed, _) => false,
        (_, Answer::ChangeRequest | Answer::Stop) => true,
        (QKind::AuditDisagreement | QKind::EffectFailed, Answer::Continue) => true,
        (QKind::CantMeet, Answer::TryAgain) => true,
        (QKind::OutOfCredit, Answer::AddCredit(_)) => true,
        _ => false,
    },
] {
    match (kind, answer) {
        (QKind::CleanupFailed, Answer::Retry | Answer::MarkHandled) => true,
        (QKind::CleanupFailed, _) => false,
        (_, Answer::ChangeRequest | Answer::Stop) => true,
        (QKind::AuditDisagreement | QKind::EffectFailed, Answer::Continue) => true,
        (QKind::CantMeet, Answer::TryAgain) => true,
        (QKind::OutOfCredit, Answer::AddCredit(_)) => true,
        _ => false,
    }
}
}

kernel_fn! {
/// Cleanup question row / #771: handling cleanup allocates no monotone counter.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
pub(super) fn mark_handled(m: &mut Node) -> (result: ()) [
    ensures final(m).effect == old(m).effect,
        final(m).ticket == old(m).ticket,
        final(m).holds == old(m).holds,
        final(m).paused == old(m).paused,
        final(m).attempt == old(m).attempt,
        final(m).round == old(m).round,
        final(m).replanned == old(m).replanned,
        final(m).plan_retries == old(m).plan_retries,
        final(m).audit_rounds == old(m).audit_rounds,
        final(m).early_end == old(m).early_end,
        final(m).run_failures == old(m).run_failures,
        final(m).run_incidents == old(m).run_incidents,
        final(m).fx_failures == old(m).fx_failures,
        final(m).fx_incidents == old(m).fx_incidents,
        final(m).waiting_incident == old(m).waiting_incident,
        final(m).incident_needs_rob == old(m).incident_needs_rob,
        final(m).proposed == old(m).proposed,
        final(m).adopted == old(m).adopted,
        final(m).head == old(m).head,
        final(m).checked == old(m).checked,
        final(m).approved == old(m).approved,
        final(m).merged == old(m).merged,
        final(m).had_pr == old(m).had_pr,
        final(m).rechecks == old(m).rechecks,
        final(m).review_restarted == old(m).review_restarted,
        final(m).priority == old(m).priority,
        final(m).review_stop == old(m).review_stop,
        final(m).planning_started == old(m).planning_started,
        final(m).waiting_prerequisite == old(m).waiting_prerequisite,
        final(m).pos == if matches!(current_item(*old(m)), Some(Item::Teardown(_))) { (old(m).pos + 1) as u8 } else { old(m).pos },
        final(m).run == if matches!(current_item(*old(m)), Some(Item::Teardown(EffectKind::StopRun))) { None } else { old(m).run },
        final(m).pool == if matches!(current_item(*old(m)), Some(Item::Teardown(EffectKind::StopRun))) { false } else { old(m).pool },
        final(m).job == if matches!(current_item(*old(m)), Some(Item::Teardown(EffectKind::WithdrawJob))) { None } else { old(m).job },
        final(m).pr_open == if matches!(current_item(*old(m)), Some(Item::Teardown(EffectKind::ClosePr))) { false } else { old(m).pr_open },
        final(m).worktree == if matches!(current_item(*old(m)), Some(Item::Teardown(EffectKind::RemoveWorktree))) { false } else { old(m).worktree },
        (old(m).effect.is_none() && old(m).question.is_none() && !old(m).waiting_incident
            && old(m).phase is Stopping && matches!(current_item(*old(m)), Some(Item::Teardown(_)))
            ==> distance(*final(m)) + 2 <= distance(*old(m))),
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation == old(m).generation,
        final(m).next_seq == old(m).next_seq, final(m).credit == old(m).credit,
        final(m).question == old(m).question,
        final(m).phase == old(m).phase,
        final(m).last_cmd == old(m).last_cmd,
] {
    kernel_snapshot! { before_handled = *m }
    if let Some(Item::Teardown(kind)) = m.current() {
        match kind {
            EffectKind::StopRun => {
                m.run = None;
                m.pool = false;
            }
            EffectKind::WithdrawJob => m.job = None,
            EffectKind::ClosePr => m.pr_open = false,
            EffectKind::RemoveWorktree => m.worktree = false,
            _ => {}
        }
        m.pos += 1;
    }
    kernel_proof! {
        let before = before_handled@;
        if before.effect.is_none() && before.question.is_none()
            && !before.waiting_incident && before.phase is Stopping
            && matches!(current_item(before), Some(Item::Teardown(_))) {
            cleanup_lemmas::teardown_progress(before_handled@, *m);
        }
    }
}
}
