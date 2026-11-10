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
    ensures *final(m) == handler_safety::handled_node(*old(m)),
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
        final(m).proposed == old(m).proposed, final(m).adopted == old(m).adopted,
        final(m).review_stop == old(m).review_stop, final(m).attempt == old(m).attempt,
        final(m).replanned == old(m).replanned,

        (final(m).holds ==> old(m).holds),
        (final(m).pool ==> old(m).pool),
        final(m).ticket == old(m).ticket || final(m).ticket.is_none(),
        final(m).credit.withheld == old(m).credit.withheld,
] {
    kernel_snapshot! { before_handled = *m }
    if let Some(Item::Teardown(kind)) = m.current() {
        mark_handled_resources(m, kind);
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

kernel_fn! {
/// Resource writes used when marking a teardown item handled.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn mark_handled_resources(m: &mut Node, kind: EffectKind) -> (result: ()) [
    ensures *final(m) == handler_safety::handled_resources_node(*old(m), kind),
        final(m).phase == old(m).phase,
        final(m).repo == old(m).repo,
        final(m).footprint == old(m).footprint,
        final(m).generation == old(m).generation,
        final(m).next_seq == old(m).next_seq,
        final(m).effect == old(m).effect,
        final(m).ticket == old(m).ticket,
        final(m).holds == old(m).holds,
        final(m).paused == old(m).paused,
        final(m).last_cmd == old(m).last_cmd,
        final(m).question == old(m).question,
        final(m).waiting_incident == old(m).waiting_incident,
        final(m).credit == old(m).credit,
        final(m).pos == old(m).pos,
        final(m).run == if kind == EffectKind::StopRun { None } else { old(m).run },
        final(m).pool == if kind == EffectKind::StopRun { false } else { old(m).pool },
        final(m).job == if kind == EffectKind::WithdrawJob { None } else { old(m).job },
        final(m).pr_open == if kind == EffectKind::ClosePr { false } else { old(m).pr_open },
        final(m).worktree == if kind == EffectKind::RemoveWorktree { false } else { old(m).worktree },
        final(m).proposed == old(m).proposed, final(m).adopted == old(m).adopted,
        final(m).review_stop == old(m).review_stop, final(m).attempt == old(m).attempt,
        final(m).replanned == old(m).replanned,
] {
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
}
}
