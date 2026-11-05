//! State-table Resume / #806: compose the real command handler's exact frame.
use super::progress_gate_apply::*;
use super::*;

verus! {
/// #806: lift the verified handler's local Resume frame into the apply frame.
/// Arbitrary finite node storage; no rank or handler behavior is re-proved.
pub(super) proof fn compose(before: World, after: World, node: NodeId, id: u64)
    requires node.0 < before.nodes@.len(),
        after.nodes@.len() == before.nodes@.len(),
        resume_post(before.nodes@[node.0 as int], after.nodes@[node.0 as int], id, Command::Resume),
        forall|j: int| 0 <= j < before.nodes@.len() && j != node.0
            ==> after.nodes@[j] == before.nodes@[j],
        after.next_question == before.next_question,
        after.file_sets@ == before.file_sets@, after.dependencies@ == before.dependencies@,
    ensures resume_apply_post(before, after, Event::Command { node, id, command: Command::Resume }),
{
    reveal(resume_post); reveal(resume_apply_post);
    assert(after.nodes@ =~= before.nodes@.update(node.0 as int, after.nodes@[node.0 as int]));
}
}
