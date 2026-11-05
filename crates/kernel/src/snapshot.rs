//! Observational copy used by real settling and proof compositions.
use super::*;

kernel_fn! {
/// #771: an observational world snapshot, including every interned file list.
/// Copy fields keep their values; nested vectors keep their element sequences.
pub(super) fn snapshot_world(world: &World) -> (result: World) [
    ensures (settle_ready(*world) ==> settle_ready(result)),
        result.nodes@ == world.nodes@, result.gates@ == world.gates@,
        result.dependencies@ == world.dependencies@,
        result.file_sets@.len() == world.file_sets@.len(),
        forall|i: int| 0 <= i < world.file_sets@.len()
            ==> result.file_sets@[i]@ == world.file_sets@[i]@,
        result.pool_capacity == world.pool_capacity,
        result.next_ticket == world.next_ticket, result.next_question == world.next_question,
        result.fail_fast == world.fail_fast,
] {
    let mut file_sets: Vec<Vec<FileId>> = Vec::new();
    let mut index = 0usize;
    kernel_while! { (index < world.file_sets.len()) [
        invariant index <= world.file_sets.len(), file_sets@.len() == index,
            forall|i: int| 0 <= i < index ==> file_sets@[i]@ == world.file_sets@[i]@,
        decreases world.file_sets.len() - index,
    ] {
        file_sets.push(world.file_sets[index].clone());
        index += 1;
    } }
    let result = World {
        nodes: world.nodes.clone(), gates: world.gates.clone(),
        dependencies: world.dependencies.clone(), file_sets,
        pool_capacity: world.pool_capacity, next_ticket: world.next_ticket,
        next_question: world.next_question, fail_fast: world.fail_fast,
    };
    kernel_proof! {
        if settle_ready(*world) {
            assert forall|j: int| 0 <= j < result.nodes@.len() implies
                footprint::valid_footprint(result, result.nodes@[j].footprint) by {
                footprint::valid_under_table_growth(*world, result, result.nodes@[j].footprint);
            }
            reveal(footprint::ordered_table);
        }
    }
    result
}
}
