//! Q48 / #806: separately gated original-domain executable cover construction.
use super::progress_gate_leaf_witness::*;
use super::*;

kernel_fn! {
/// Q48: original-domain Node::new construction, four nodes, one gate, four files.
/// Case 0 starts full; case 1 permits a disjoint bypass; case 2 passes an older
/// waiter which is itself blocked by an overlapping oldest waiter.
pub(super) fn leaf_world(width: u8, case: u8) -> (result: World) [
    requires width == 2 || width == 3, case < 3,
    ensures cover_input(result, width, case),
] {
    let mut first = Node::new(RepoId(0));
    kernel_snapshot! { seed = first }
    first.phase = Phase::Ready(Resume::Making);
    first.credit.ready = true;
    first.credit.start = 4;
    first.attempt = 1;
    first.footprint = Footprint::inline(if case == 2 { 3 } else { 1 });
    first.ticket = Some(1);
    let mut blocker = first;
    blocker.phase = Phase::Queued;
    blocker.ticket = None;
    blocker.footprint = Footprint::inline(1);
    blocker.holds = true;
    blocker.head = Some(HeadId(1));
    blocker.checked = blocker.head;
    blocker.approved = blocker.head;
    blocker.had_pr = true;
    blocker.pr_open = true;
    blocker.worktree = true;
    let mut second = first;
    second.footprint = Footprint::inline(2);
    second.ticket = Some(2);
    let mut third = first;
    third.footprint = Footprint::inline(4);
    third.ticket = Some(3);
    if case == 0 {
        second = blocker;
        second.footprint = Footprint::inline(2);
        if width == 3 {
            third = blocker;
            third.footprint = Footprint::inline(4);
        }
    }
    let mut nodes = Vec::new();
    nodes.push(first); nodes.push(blocker); nodes.push(second); nodes.push(third);
    let mut gates = Vec::new();
    gates.push(Gate { w: width });
    let w = World { nodes, gates, pool_capacity: 4, next_ticket: 8, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    kernel_proof! {
        assert(w.nodes@ =~= progress_gate_leaf_construct::cover_nodes(seed@, width, case));
        assert(w.gates@ =~= seq![Gate { w: width }]);
        progress_gate_leaf_construct::construction_valid(w, seed@, width, case);
    }
    let checked = w.check();
    assert!(checked.is_ok());
    if case == 0 {
        kernel_proof! { assert(retirement_wide::holders(w, RepoId(0), 4) == width); }
    }
    if case == 2 {
        // The exact original chained_waiter condition, evaluated with real queries.
        assert!(matches!(w.nodes[2].phase, Phase::Ready(_)));
        assert!(matches!(w.nodes[3].phase, Phase::Ready(_)));
        assert!(slot_free(&w, 2));
        assert!(w.nodes[2].ticket.unwrap() < w.nodes[3].ticket.unwrap());
        assert!(!w.footprints_disjoint(w.nodes[0].footprint, w.nodes[2].footprint));
        assert!(w.footprints_disjoint(w.nodes[0].footprint, w.nodes[3].footprint));
        assert!(w.footprints_disjoint(w.nodes[2].footprint, w.nodes[3].footprint));
    }
    w
}
}
