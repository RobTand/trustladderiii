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
    ensures (old(m).effect.is_none() && old(m).question.is_none() && !old(m).waiting_incident
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
] {
    kernel_snapshot! { before_handled = *m }
    if let Some(Item::Teardown(kind)) = m.current() {
        kernel_snapshot! { before_resources_call = *m }
        mark_handled_resources(m, kind);
        kernel_proof! {
            node_frame_groups_reveal::mark_handled_resources_rows(before_resources_call@, *m, kind);
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

kernel_fn! {
/// Resource writes used when marking a teardown item handled.
#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]
fn mark_handled_resources(m: &mut Node, kind: EffectKind) -> (result: ()) [
    ensures node_frame_groups::ops_same(*old(m), *final(m)),
        node_frame_groups::ledger_same(*old(m), *final(m)),
        node_frame_groups::counters_same(*old(m), *final(m)),
        node_frame_groups::pr_same(*old(m), *final(m)),
        final(m).run == if kind == EffectKind::StopRun { None } else { old(m).run },
        final(m).pool == if kind == EffectKind::StopRun { false } else { old(m).pool },
        final(m).job == if kind == EffectKind::WithdrawJob { None } else { old(m).job },
        final(m).pr_open == if kind == EffectKind::ClosePr { false } else { old(m).pr_open },
        final(m).worktree == if kind == EffectKind::RemoveWorktree { false } else { old(m).worktree },
] {
    kernel_snapshot! { before_resources = *m }
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
    kernel_proof! {
        node_frame_groups_reveal::frame_intro(before_resources@, *m);
    }
}
}
