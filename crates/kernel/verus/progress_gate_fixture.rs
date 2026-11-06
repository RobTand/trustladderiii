//! #806: executable original-domain constructors shared with C2 and Q48 leaf ports.
use super::progress_gate::*;
use super::*;

kernel_fn! {
/// #806 gate_world: build one original leaf with symbolic phase, files, pause and ticket.
pub(super) fn make_leaf(kind: u8, files: u64, paused: bool, ticket: Option<u64>) -> (n: Node) [
    requires kind < 4, 0 < files < 16,
    ensures leaf(n), n.footprint == footprint::inline_footprint(files), n.paused == paused,
        n.phase == match kind { 0 => Phase::Ready(Resume::Making), 1 => Phase::Queued,
            2 => Phase::Making, _ => Phase::Unplanned },
        n.ticket == if kind == 0 { ticket } else { None },
] proof [reveal(initial_node);] {
    let mut n = Node::new(RepoId(0));
    n.credit.ready = true;
    n.credit.start = 4;
    n.attempt = 1;
    n.footprint = Footprint::inline(files);
    n.paused = paused;
    n.phase = match kind { 0 => Phase::Ready(Resume::Making), 1 => Phase::Queued,
        2 => Phase::Making, _ => Phase::Unplanned };
    if kind == 1 {
        n.holds = true;
        n.head = Some(HeadId(1));
        n.checked = n.head;
        n.approved = n.head;
        n.had_pr = true;
        n.pr_open = true;
        n.worktree = true;
    } else if kind == 2 {
        n.holds = true;
        n.worktree = true;
        n.pos = 2;
        n.run = Some(n.generation);
        n.pool = true;
    } else if kind == 0 {
        n.ticket = ticket;
    }
    n
}
}

kernel_fn! {
/// #806: the exact four-node/one-repository storage constructor, reusable without phase coupling.
pub(super) fn assemble(width: u8, a: Node, b: Node, c: Node, d: Node) -> (w: World) [
    requires width > 0, leaf(a), leaf(b), leaf(c), leaf(d),
    ensures domain(w), w.nodes@ == seq![a, b, c, d], w.gates@.len() == 1 && w.gates@[0].w == width,
        w.pool_capacity == 4 && w.next_ticket == 8 && w.next_question == 0 && !w.fail_fast,
        world_headroom(w), storage_ordered(w),
] proof [reveal(initial_node); reveal(world_headroom); reveal(storage_ordered);
    reveal(footprint::ordered_table); reveal_with_fuel(open_prefix, 5);] {
    let mut nodes = Vec::new();
    nodes.push(a); nodes.push(b); nodes.push(c); nodes.push(d);
    let mut gates = Vec::new();
    gates.push(Gate { w: width });
    let w = World { nodes, gates, pool_capacity: 4, next_ticket: 8, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    kernel_proof! {
        assert forall|i: int| 0 <= i < 4 implies node_headroom(w.nodes@[i]) by {
            assert(i == 0 || i == 1 || i == 2 || i == 3);
        }
    }
    w
}
}
