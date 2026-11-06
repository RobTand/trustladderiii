//! #806: shared-rule composition for complete reply admission frames.
use super::port_replies_contracts::*;
use super::port_replies_frames::*;
use super::*;

verus! {
/// #806: the full before/after node rules asserted by check_apply for both nodes.
pub(super) open spec fn shared(before: World, after: World, reply: Reply) -> bool {
    after.nodes@.len() == before.nodes@.len()
        && forall|i: int| 0 <= i < before.nodes@.len() ==>
            node_change(before.nodes@[i], after.nodes@[i], before.fail_fast, reply)
}

/// #806: derive the real apply and complete-checker premises from the same or
/// weaker original-domain construction; no extra invariant is assumed by callers.
pub(super) proof fn apply_scope(w: World)
    requires retirement::domain(w), retirement::accepted(w), retirement::bounded(w), storage_ordered(w),
    ensures world_headroom(w), footprint::ordered_table(w), dependencies::edges_indexable(w),
        forall|i: int| 0 <= i < w.nodes@.len() ==> node_headroom(w.nodes@[i])
            && arithmetic_consistent(w.nodes@[i]) && settle_shape(w.nodes@[i])
            && stopping_effect_fits(w.nodes@[i])
            && (w.nodes@[i].phase is Closed ==> w.nodes@[i].effect.is_none())
            && footprint::valid_footprint(w, w.nodes@[i].footprint),
{
    reveal(retirement::accepted); reveal(retirement::node_rules); reveal(retirement::bounded);
    reveal(storage_ordered); reveal(world_headroom);
    open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
    reveal_with_fuel(open_prefix, 3);
    assert forall|i: int| 0 <= i < w.nodes@.len() implies node_headroom(w.nodes@[i]) by {}
    dependencies::indexable_definition(w);
}

/// #806: original two-node counter bounds leave headroom for the real complete
/// checker after one apply, including the largest ingest deposit.
pub(super) proof fn checker_headroom(before: World, after: World)
    requires retirement::domain(before), retirement::bounded(before), after.nodes@.len() == 2,
        after.next_ticket == before.next_ticket,
        after.next_question as int <= before.next_question as int + 1,
        forall|i: int| 0 <= i < 2 ==> {
            let b = before.nodes@[i]; let n = #[trigger] after.nodes@[i];
            n.generation as int <= b.generation as int + 2
                && n.next_seq as int <= b.next_seq as int + 1
                && n.credit.start as int <= b.credit.start as int + CREDIT_START_STEP_BUDGET
                && n.credit.earned as int <= b.credit.earned as int + 1
                && n.credit.added as int <= b.credit.added as int + CREDIT_ADDED_STEP_BUDGET
                && n.credit.spent as int <= b.credit.spent as int + CREDIT_SPENT_STEP_BUDGET
        },
    ensures world_headroom(after),
{
    reveal(retirement::bounded); reveal(world_headroom);
    open_prefix_monotone(after.nodes@, 0, 2);
    reveal(open_prefix);
    assert forall|i: int| 0 <= i < 2 implies node_headroom(after.nodes@[i]) by {
        assert(node_headroom(before.nodes@[i]));
    }
}

/// #806: choose one small frame lemma for each admitted payload, without
/// expanding the world or re-encoding the old bounded Kani construction.
pub(super) proof fn admitted_node(before: World, after: World, reply: Reply)
    requires selected(reply), local_context(before, after, before.nodes@[0], reply),
        after.nodes@[0] == transitioned(ended(before.nodes@[0]), before.next_question, before.fail_fast, reply),
    ensures retirement::node_rules(after, after.nodes@[0]),
        node_change(before.nodes@[0], after.nodes@[0], before.fail_fast, reply),
        !after.nodes@[0].holds, !after.nodes@[0].pool, after.nodes@[0].ticket.is_none(),
        !after.nodes@[0].credit.withheld, after.nodes@[0].repo == before.nodes@[0].repo,
{
    let n = before.nodes@[0];
    match reply {
        Reply::IngestDone { size } => ingest_done(before, after, n, size),
        Reply::IngestEarlyEnd(e) => ingest_early_end(before, after, n, e),
        Reply::ProtectedPaths => protected_paths(before, after, n),
        Reply::AuditAgrees => audit_agrees(before, after, n),
        Reply::AuditDisagrees => audit_disagrees(before, after, n),
        Reply::PlanLeaf { footprint, fits } => plan_leaf(before, after, n, footprint, fits),
        Reply::PlanSplit => plan_split(before, after, n),
        Reply::PlanCantMeet => plan_cant_meet(before, after, n),
        _ => {},
    }
}
}
