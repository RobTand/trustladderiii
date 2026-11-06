//! Q48 / #806: project the rebuilt real advance contract onto the leaf lemmas.
use super::*;

verus! {
/// State-table teardown and Making rows: reuse the real dispatcher's exact
/// relation while leaving every production advance contract unchanged.
pub(super) proof fn local_details(before: Node, after: Node, ticket: u64, next: u64,
    context: AdvanceContext, out_before: Outbox, out_after: Outbox)
    requires port_advance_contracts::exact(before, after, ticket, next,
        context, out_before, out_after),
    ensures progress_gate_leaf_steps::advance_details(before, after, ticket, next),
        before.phase == Phase::Queued && !before.waiting_incident
            ==> !advance_enabled(before, context),
{
    reveal(port_advance_contracts::exact);
    reveal(port_advance_contracts::node_after);
    reveal(progress_gate_leaf_steps::advance_details);
}
}
