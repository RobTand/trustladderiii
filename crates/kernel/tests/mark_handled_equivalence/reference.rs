//! Verbatim pre-extraction body for marking a teardown item handled.
use super::*;

pub(super) fn mark_handled(m: &mut Node) {
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
