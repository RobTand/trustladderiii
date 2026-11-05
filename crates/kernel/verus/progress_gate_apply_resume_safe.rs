//! State-table gate apply / #806: complete consistency after the real update.
use super::progress_gate_apply::*;
use super::*;

verus! {
/// State-table Resume / #806: clearing a pause and recording a fresh ID preserve
/// every checker rule. Scheduling itself occurs later, outside these harnesses.
pub(super) proof fn resume_safe(before: World, after: World, id: u64)
    requires resume_input(before),
        after.nodes@ == before.nodes@.update(0, Node { paused: false, last_cmd: id, ..before.nodes@[0] }),
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket, after.next_question == before.next_question,
        after.pool_capacity == before.pool_capacity, after.dependencies@ == before.dependencies@,
    ensures domain(after), accepted(after),
{
    reveal(accepted); reveal(retirement::node_rules);
    reveal_with_fuel(potential_prefix, 5);
    assert forall|j: int| 0 <= j < 4 implies retirement::node_rules(after, after.nodes@[j]) by {
        retirement::node_frame(before, after, before.nodes@[j]);
    }
    assert forall|a: int, b: int| 0 <= a < b < 4 implies pair(after, a, b) by {
        assert(pair(before, a, b));
        footprint::storage_frame(before, after, before.nodes@[a].footprint, before.nodes@[b].footprint);
    }
    assert forall|r: int| 0 <= r < after.gates@.len() implies
        #[trigger] holders(after, RepoId(r as u64), 4) == holders(before, RepoId(r as u64), 4) by {}
}}
