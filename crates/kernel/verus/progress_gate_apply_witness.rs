//! #806: executable original-domain merge and resume cover constructors.
use super::progress_gate_apply::*;
use super::*;

kernel_fn! {
/// #806 gate_world: four nodes, one repo, pool=4, next_ticket=8, credit=4,
/// attempt=1 and inline files in 1..15. Both original widths are instantiated.
fn original_world(width: u8, merge: bool) -> (result: World) [
    requires width == 2 || width == 3,
    ensures gate_world(result), result.gates@[0].w == width,
        merge ==> merge_input(result), !merge ==> resume_input(result),
        result.next_ticket == 8 && result.pool_capacity == 4,
        forall|j: int| 0 <= j < 4 ==> result.nodes@[j].credit.ready
            && result.nodes@[j].credit.start == 4 && result.nodes@[j].attempt == 1,
] proof [
    reveal(initial_node); reveal(accepted); reveal(retirement::node_rules);
    reveal(retirement::bounded); reveal(storage_ordered); reveal(fresh_commands);
    reveal(footprint::ordered_table); reveal_with_fuel(potential_prefix, 5);
] {
    let mut nodes = Vec::new();
    let mut first = Node::new(RepoId(0));
    first.credit.ready = true;
    first.credit.start = 4;
    first.attempt = 1;
    first.footprint = Footprint::inline(1);
    if merge {
        first.phase = Phase::Queued;
        first.holds = true;
        first.head = Some(HeadId(1));
        first.checked = first.head;
        first.approved = first.head;
        first.had_pr = true;
        first.pr_open = true;
        first.worktree = true;
    } else {
        first.phase = Phase::Unplanned;
        first.paused = true;
    }
    nodes.push(first);
    let mut i = 1usize;
    kernel_while! { (i < 4) [
        invariant 1 <= i <= 4, nodes.len() == i, nodes@[0] == first,
            forall|j: int| 1 <= j < i ==> initial_node(Node {
                phase: Phase::Ingesting, credit: Credit { ready: false, start: 0, ..nodes@[j].credit },
                attempt: 0, footprint: footprint::inline_footprint(0), ..nodes@[j]
            }, RepoId(0)) && nodes@[j].phase == Phase::Unplanned
                && nodes@[j].credit.ready && nodes@[j].credit.start == 4
                && nodes@[j].attempt == 1 && nodes@[j].footprint == footprint::inline_footprint(2),
        decreases 4 - i,
    ] {
        let mut n = Node::new(RepoId(0));
        n.phase = Phase::Unplanned;
        n.credit.ready = true;
        n.credit.start = 4;
        n.attempt = 1;
        n.footprint = Footprint::inline(2);
        nodes.push(n);
        i += 1;
    } }
    let mut gates = Vec::new();
    gates.push(Gate { w: width });
    let world = World { nodes, gates, pool_capacity: 4, next_ticket: 8, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    kernel_proof! {
        footprint::inline_is_valid(world, 1);
        footprint::inline_is_valid(world, 2);
        assert forall|j: int| 0 <= j < 4 implies retirement::node_rules(world, world.nodes@[j]) by {
            if j > 0 { assert(initial_node(Node {
                phase: Phase::Ingesting, credit: Credit { ready: false, start: 0, ..world.nodes@[j].credit },
                attempt: 0, footprint: footprint::inline_footprint(0), ..world.nodes@[j]
            }, RepoId(0))); }
        }
        assert(accepted(world));
    }
    world
}
}

kernel_fn! {
/// #806 covers: original W2/W3 worlds call real apply through the universal
/// wrappers; each returned world witnesses the strict natural-measure decrease.
fn cover_apply(width: u8, merge: bool) -> (result: (World, Ghost<World>)) [
    requires width == 2 || width == 3,
    ensures progress_measure(result.0) < progress_measure(result.1@),
        accepted(result.1@), accepted(result.0), result.1@.gates@[0].w == width,
] {
    let before = original_world(width, merge);
    kernel_snapshot! { original = before }
    let (after, _out) = if merge {
        progress_gate_apply_merge::verify_merge(before)
    } else {
        progress_gate_apply_resume::verify_resume(before)
    };
    (after, original)
}
}
