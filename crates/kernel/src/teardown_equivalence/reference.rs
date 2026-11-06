//! #806: verbatim pre-refactor advance_teardown body at 37dfb232ae.
use super::super::*;

pub(super) fn advance_teardown(
    node: &mut Node,
    _next_ticket: &mut u64,
    context: AdvanceContext,
    out: &mut Outbox,
    kind: EffectKind,
) -> bool {
    kernel_snapshot! { advance_out = *out }
    let n = *node;
    if n.question.is_some() {
        return false; // waiting for Rob
    }
    kernel_proof! { assert(n.phase is Stopping); }
    let gone = match kind {
        EffectKind::StopRun => n.run.is_none(),
        EffectKind::WithdrawJob => n.job.is_none(),
        EffectKind::ClosePr => !n.pr_open,
        EffectKind::RemoveWorktree => !n.worktree,
        _ => false,
    };
    if gone {
        let m = &mut *node;
        if kind == EffectKind::StopRun {
            m.pool = false;
        }
        m.pos += 1;
        return true;
    }
    emit_node(node, context.node, kind, out);
    kernel_proof! { port_advance_step_frames::teardown_emit(n, *_next_ticket, context, advance_out@, *out, kind); }
    true
}
