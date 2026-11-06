//! #806: derive the original output assertions from exact advance contracts.
use super::*;

verus! {
/// State-table effect machine / Q41: fresh keys, pause rules, and exact Enqueue heads.
pub(super) open spec fn effect_rule(before: Node, after: Node, node: usize, output: Out) -> bool {
    match output {
        Out::Effect { node: id, seq, kind, head } => id == NodeId(node as u64)
            && before.next_seq <= seq < after.next_seq
            && (before.paused && !(before.phase is Stopping) ==> matches!(kind,
                EffectKind::StopRun | EffectKind::WithdrawJob | EffectKind::ClosePr | EffectKind::RemoveWorktree))
            && (kind == EffectKind::Enqueue ==> match head {
                Some(h) => after.approved == Some(h) && after.checked == Some(h), None => false,
            }),
        Out::Notice { .. } => true,
    }
}

/// #806: every original check_effect assertion follows for every occupied output slot.
pub(super) proof fn output_rules(w: World, before: Node, after: Node, ticket_before: u64,
    ticket_after: u64, c: AdvanceContext, out_before: Outbox, out_after: Outbox)
    requires retirement::node_rules(w, before), advance_pre(before, ticket_before, out_before.len),
        port_advance_contracts::exact(before, after, ticket_before, ticket_after, c, out_before, out_after),
        empty_outbox(out_before),
    ensures forall|slot: int| 0 <= slot < OUTBOX ==> match #[trigger] out_after.items@[slot] {
        Some(output) => effect_rule(before, after, c.node, output), None => true,
    },
{
    reveal(empty_outbox);
    reveal(port_advance_contracts::exact);
    reveal(port_advance_contracts::outputs);
    reveal(port_advance_contracts::effect_kind);
    reveal(port_advance_contracts::node_after);
    reveal(retirement::node_rules); reveal(node_checked); reveal(ready_checked);
    assert forall|slot: int| 0 <= slot < OUTBOX implies match #[trigger] out_after.items@[slot] {
        Some(output) => effect_rule(before, after, c.node, output), None => true,
    } by {
        assert(out_before.items@[slot].is_none());
    }
}

/// #806: a false result of the real dispatcher has no output, including notices.
pub(super) proof fn stutter(n: Node, c: AdvanceContext, before: Outbox, after: Outbox)
    requires !advance_enabled(n, c), port_advance_contracts::outputs(n, c, before, after),
    ensures after == before,
{
    reveal(port_advance_contracts::outputs); reveal(port_advance_contracts::effect_kind);
}

}
