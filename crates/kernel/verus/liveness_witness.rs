//! Concrete inhabitants of both original Kani gate-leaf input worlds.
use super::*;

verus! {
pub(super) fn width_2() -> (w: World)
    ensures liveness_trace::initial(w, 0), w.gates@[0].w == 2,
{
    let mut n0 = Node::new(RepoId(0));
    n0.credit.ready = true;
    n0.credit.start = 4;
    n0.attempt = 1;
    n0.footprint = Footprint::inline(1);
    n0.phase = Phase::Ready(Resume::Making);
    n0.ticket = Some(0);
    let mut n1 = Node::new(RepoId(0));
    n1.credit.ready = true;
    n1.credit.start = 4;
    n1.attempt = 1;
    n1.footprint = Footprint::inline(1);
    n1.phase = Phase::Rebasing;
    n1.holds = true;
    n1.pos = 1;
    n1.run = Some(0);
    n1.head = Some(HeadId(1));
    n1.checked = Some(HeadId(1));
    n1.approved = Some(HeadId(1));
    n1.had_pr = true;
    n1.pr_open = true;
    n1.worktree = true;
    let mut n2 = Node::new(RepoId(0));
    n2.credit.ready = true;
    n2.credit.start = 4;
    n2.attempt = 1;
    n2.footprint = Footprint::inline(2);
    n2.phase = Phase::Rebasing;
    n2.holds = true;
    n2.pos = 1;
    n2.run = Some(0);
    n2.head = Some(HeadId(1));
    n2.checked = Some(HeadId(1));
    n2.approved = Some(HeadId(1));
    n2.had_pr = true;
    n2.pr_open = true;
    n2.worktree = true;
    let mut n3 = Node::new(RepoId(0));
    n3.credit.ready = true;
    n3.credit.start = 4;
    n3.attempt = 1;
    n3.footprint = Footprint::inline(4);
    n3.phase = Phase::Ready(Resume::Making);
    n3.ticket = Some(3);
    let mut nodes = Vec::new();
    nodes.push(n0); nodes.push(n1); nodes.push(n2); nodes.push(n3);
    let mut gates = Vec::new();
    gates.push(Gate { w: 2 });
    let w = World { nodes, gates, pool_capacity: 4, next_ticket: 8, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    proof {
        reveal(initial_node);
        reveal(liveness_bounds::context); reveal(liveness_bounds::small);
        reveal(liveness_trace::initial); reveal(liveness_trace::initial_young);
        reveal(liveness_reservation::holder); reveal(footprint::ordered_table);
        reveal(retirement::node_rules);
        footprint::inline_is_valid(w, 1);
        footprint::inline_is_valid(w, 2);
        footprint::inline_is_valid(w, 4);
        assert((1u64 & 1u64) == 1 && (1u64 & 2u64) == 0 && (1u64 & 4u64) == 0
            && (2u64 & 1u64) == 0 && (2u64 & 4u64) == 0 && (4u64 & 1u64) == 0) by(bit_vector);
        footprint::inline_disjoint(w, 1, 1);
        footprint::inline_disjoint(w, 1, 2);
        footprint::inline_disjoint(w, 1, 4);
        footprint::inline_disjoint(w, 2, 1);
        footprint::inline_disjoint(w, 2, 4);
        footprint::inline_disjoint(w, 4, 1);
        assert forall|i: int| 0 <= i < 4 implies retirement::node_rules(w, w.nodes@[i]) by {
            assert(i == 0 || i == 1 || i == 2 || i == 3);
        }
        dependencies::rules_definition(w);
        dependencies::acyclic_definition(w);
        reveal(dependencies::active_edge);
        assert forall|i: int| 0 <= i < 4 implies !w.nodes@[i].credit.withheld by {
            assert(i == 0 || i == 1 || i == 2 || i == 3);
        }
        checking::accepted_definition(w);
        reveal_with_fuel(potential_prefix, 5);
        assert forall|a: int, b: int| 0 <= a < b < 4 implies progress_gate_apply::pair(w, a, b) by {
            assert(a == 0 || a == 1 || a == 2);
            assert(b == 1 || b == 2 || b == 3);
        }
        assert(checking::accepted(w));
        assert forall|i: int| 0 <= i < 4 implies liveness_bounds::small(w.nodes@[i], 8) by {
            assert(i == 0 || i == 1 || i == 2 || i == 3);
        }
        assert(liveness_bounds::context(w, 8));
        assert(current_item(w.nodes@[2]) == Some(Item::Await));
        assert(liveness_reservation::holder(w.nodes@[2]));
        assert(footprint::disjoint(w, w.nodes@[2].footprint, w.nodes@[0].footprint));
        assert(liveness_trace::initial_young(w, 2, 0));
        assert(liveness_trace::initial_young(w, 3, 0));
        assert(liveness_trace::initial(w, 0));
    }
    w
}

pub(super) fn width_3() -> (w: World)
    ensures liveness_trace::initial(w, 0), w.gates@[0].w == 3,
{
    let mut n0 = Node::new(RepoId(0));
    n0.credit.ready = true;
    n0.credit.start = 4;
    n0.attempt = 1;
    n0.footprint = Footprint::inline(1);
    n0.phase = Phase::Ready(Resume::Making);
    n0.ticket = Some(0);
    let mut n1 = Node::new(RepoId(0));
    n1.credit.ready = true;
    n1.credit.start = 4;
    n1.attempt = 1;
    n1.footprint = Footprint::inline(1);
    n1.phase = Phase::Rebasing;
    n1.holds = true;
    n1.pos = 1;
    n1.run = Some(0);
    n1.head = Some(HeadId(1));
    n1.checked = Some(HeadId(1));
    n1.approved = Some(HeadId(1));
    n1.had_pr = true;
    n1.pr_open = true;
    n1.worktree = true;
    let mut n2 = Node::new(RepoId(0));
    n2.credit.ready = true;
    n2.credit.start = 4;
    n2.attempt = 1;
    n2.footprint = Footprint::inline(2);
    n2.phase = Phase::Rebasing;
    n2.holds = true;
    n2.pos = 1;
    n2.run = Some(0);
    n2.head = Some(HeadId(1));
    n2.checked = Some(HeadId(1));
    n2.approved = Some(HeadId(1));
    n2.had_pr = true;
    n2.pr_open = true;
    n2.worktree = true;
    let mut n3 = Node::new(RepoId(0));
    n3.credit.ready = true;
    n3.credit.start = 4;
    n3.attempt = 1;
    n3.footprint = Footprint::inline(4);
    n3.phase = Phase::Rebasing;
    n3.holds = true;
    n3.pos = 1;
    n3.run = Some(0);
    n3.head = Some(HeadId(1));
    n3.checked = Some(HeadId(1));
    n3.approved = Some(HeadId(1));
    n3.had_pr = true;
    n3.pr_open = true;
    n3.worktree = true;
    let mut nodes = Vec::new();
    nodes.push(n0); nodes.push(n1); nodes.push(n2); nodes.push(n3);
    let mut gates = Vec::new();
    gates.push(Gate { w: 3 });
    let w = World { nodes, gates, pool_capacity: 4, next_ticket: 8, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    proof {
        reveal(initial_node);
        reveal(liveness_bounds::context); reveal(liveness_bounds::small);
        reveal(liveness_trace::initial); reveal(liveness_trace::initial_young);
        reveal(liveness_reservation::holder); reveal(footprint::ordered_table);
        reveal(retirement::node_rules);
        footprint::inline_is_valid(w, 1);
        footprint::inline_is_valid(w, 2);
        footprint::inline_is_valid(w, 4);
        assert((1u64 & 1u64) == 1 && (1u64 & 2u64) == 0 && (1u64 & 4u64) == 0
            && (2u64 & 1u64) == 0 && (2u64 & 4u64) == 0 && (4u64 & 1u64) == 0) by(bit_vector);
        footprint::inline_disjoint(w, 1, 1);
        footprint::inline_disjoint(w, 1, 2);
        footprint::inline_disjoint(w, 1, 4);
        footprint::inline_disjoint(w, 2, 1);
        footprint::inline_disjoint(w, 2, 4);
        footprint::inline_disjoint(w, 4, 1);
        assert forall|i: int| 0 <= i < 4 implies retirement::node_rules(w, w.nodes@[i]) by {
            assert(i == 0 || i == 1 || i == 2 || i == 3);
        }
        dependencies::rules_definition(w);
        dependencies::acyclic_definition(w);
        reveal(dependencies::active_edge);
        assert forall|i: int| 0 <= i < 4 implies !w.nodes@[i].credit.withheld by {
            assert(i == 0 || i == 1 || i == 2 || i == 3);
        }
        checking::accepted_definition(w);
        reveal_with_fuel(potential_prefix, 5);
        assert forall|a: int, b: int| 0 <= a < b < 4 implies progress_gate_apply::pair(w, a, b) by {
            assert(a == 0 || a == 1 || a == 2);
            assert(b == 1 || b == 2 || b == 3);
        }
        assert(checking::accepted(w));
        assert forall|i: int| 0 <= i < 4 implies liveness_bounds::small(w.nodes@[i], 8) by {
            assert(i == 0 || i == 1 || i == 2 || i == 3);
        }
        assert(liveness_bounds::context(w, 8));
        assert(current_item(w.nodes@[2]) == Some(Item::Await));
        assert(liveness_reservation::holder(w.nodes@[2]));
        assert(footprint::disjoint(w, w.nodes@[2].footprint, w.nodes@[0].footprint));
        assert(liveness_trace::initial_young(w, 2, 0));
        assert(liveness_trace::initial_young(w, 3, 0));
        assert(liveness_trace::initial(w, 0));
    }
    w
}

}
