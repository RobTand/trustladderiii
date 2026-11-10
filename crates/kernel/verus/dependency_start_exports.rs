//! Acceptance carriers for constructor results and append-only footprint interning.
use super::*;

verus! {
/// World::new has empty nodes and dependencies. Its other initial values do not
/// impose further checker conditions when every gate width and the pool are positive.
pub(super) proof fn new_world_accepted(w: World)
    requires w.nodes@.len() == 0, w.dependencies@.len() == 0,
        w.gates@.len() > 0,
        forall|g: int| 0 <= g < w.gates@.len() ==> w.gates@[g].w > 0,
    ensures checking::accepted(w),
{
    checking::accepted_definition(w);
    dependencies::rules_definition(w);
    dependencies::acyclic_definition(w);
    reveal(dependencies::active_edge);
    assert forall|from: NodeId, to: NodeId| dependencies::active_edge(w, from, to)
        implies !dependencies::reachable(w, to, from) by {}
}

/// intern_footprint changes only the append-only file table. These are the
/// verified intern_sorted_files per-field posts, with its old/final worlds named.
pub(super) proof fn intern_footprint_preserves_accepted(before: World, after: World)
    requires checking::accepted(before),
        after.file_sets@.len() >= before.file_sets@.len(),
        forall|j: int| 0 <= j < before.file_sets@.len()
            ==> after.file_sets@[j] == before.file_sets@[j],
        after.nodes@ == before.nodes@,
        after.gates@ == before.gates@,
        after.next_ticket == before.next_ticket,
        after.next_question == before.next_question,
        after.pool_capacity == before.pool_capacity,
        after.fail_fast == before.fail_fast,
        after.dependencies@ == before.dependencies@,
    ensures checking::accepted(after),
{
    checking::accepted_parts(before);
    checking::accepted_parts(after);
    reveal(progress_gate_apply::accepted);
    dependencies::frame_preserves(before, after);
    assert forall|j: int| 0 <= j < after.nodes@.len()
        implies retirement::node_rules(after, #[trigger] after.nodes@[j]) by {
        footprint::node_rules_grow(before, after, before.nodes@[j]);
    }
    assert forall|a: int, b: int| 0 <= a < b < after.nodes@.len()
        implies progress_gate_apply::pair(after, a, b) by {
        if after.nodes@[a].holds && after.nodes@[b].holds
            && after.nodes@[a].repo == after.nodes@[b].repo {
            checking::accepted_node(before, a);
            checking::accepted_node(before, b);
            assert(progress_gate_apply::pair(before, a, b));
            footprint::disjoint_under_table_growth(before, after,
                before.nodes@[a].footprint, before.nodes@[b].footprint);
        }
    }
}
}
