//! Teardown rank facts isolated from mutable cleanup frames.
use super::*;

verus! {
/// Stopping: skipping an absent resource strictly consumes its agenda entry.
pub(super) proof fn absent(n: Node, ticket: u64, c: AdvanceContext, kind: EffectKind)
    requires advance_pre(n, ticket, 0), n.phase is Stopping,
        !n.waiting_incident, n.effect.is_none(), n.question.is_none(),
        current_item(n) == Some(Item::Teardown(kind)),
        match kind {
            EffectKind::StopRun => n.run.is_none(),
            EffectKind::WithdrawJob => n.job.is_none(),
            EffectKind::ClosePr => !n.pr_open,
            EffectKind::RemoveWorktree => !n.worktree,
            _ => false,
        },
        pending_rank(n) == 0,
        pending_rank(Node { pool: n.pool && kind != EffectKind::StopRun,
            pos: (n.pos + 1) as u8, ..n }) == 0,
    ensures advance_post(n, Node { pool: n.pool && kind != EffectKind::StopRun,
        pos: (n.pos + 1) as u8, ..n }, ticket, ticket, advance_enabled(n, c)),
{
    reveal(advance_post); reveal(distance);
    assert(n.pos < 5);
}
}
