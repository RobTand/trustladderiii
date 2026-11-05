//! State-table prerequisite progress, #693 / #805. All sums are mathematical
//! naturals over arbitrary finite storage; there is no machine sum overflow.
use super::*;

verus! {
pub(super) closed spec fn edge_weight(nodes: Seq<Node>, edge: Dependency) -> nat {
    if edge.status != DependencyStatus::Delivered
        && !(nodes[edge.node.0 as int].phase is Closed) { 1nat } else { 0nat }
}

pub(super) closed spec fn edge_total(nodes: Seq<Node>, edges: Seq<Dependency>, end: nat) -> nat
    recommends end <= edges.len(),
    decreases end,
{
    if end == 0 { 0 } else {
        edge_total(nodes, edges, (end - 1) as nat)
            + edge_weight(nodes, edges[end as int - 1])
    }
}

/// #805: World::measure without the executable u64 sum's size restriction.
pub closed spec fn progress_measure(w: World) -> nat {
    node_total(w.nodes@) + edge_total(w.nodes@, w.dependencies@, w.dependencies@.len())
}

/// #805: delivery only reconciles wait flags, which do not enter node distance.
proof fn wait_flags_distance(before: Node, after: Node)
    requires after == (Node {
        waiting_prerequisite: after.waiting_prerequisite,
        credit: Credit { withheld: after.credit.withheld, ..before.credit }, ..before }),
    ensures distance(after) == distance(before),
{
    reveal(distance);
}

/// #805 frame: node distance and every owner's open/closed status stay equal.
proof fn delivery_node_frame(before: World, after: World, i: int)
    requires 0 <= i < before.nodes@.len(),
        dependencies::dependency_frame(before, after, i),
    ensures node_total(after.nodes@) == node_total(before.nodes@),
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len()
            ==> after.nodes@[j].phase == before.nodes@[j].phase,
{
    reveal(dependencies::dependency_frame);
    wait_flags_distance(before.nodes@[i], after.nodes@[i]);
    assert(after.nodes@ =~= before.nodes@.update(i, after.nodes@[i]));
    sum_update(before.nodes@, i, after.nodes@[i]);
}

/// #805 frame: the unchanged prefix has exactly the same prerequisite debt.
proof fn edge_prefix_frame(before: Seq<Node>, after: Seq<Node>, edges: Seq<Dependency>, end: nat)
    requires end <= edges.len(),
        forall|j: int| 0 <= j < end
            ==> edge_weight(after, edges[j]) == edge_weight(before, edges[j]),
    ensures edge_total(after, edges, end) == edge_total(before, edges, end),
    decreases end,
{
    reveal(edge_total);
    if end > 0 { edge_prefix_frame(before, after, edges, (end - 1) as nat); }
}

/// #805 delta: replacing one actual row changes exactly its summand.
proof fn edge_update_delta(nodes: Seq<Node>, edges: Seq<Dependency>, k: int,
    replacement: Dependency, end: nat)
    requires 0 <= k < end <= edges.len(),
    ensures edge_total(nodes, edges.update(k, replacement), end) + edge_weight(nodes, edges[k])
        == edge_total(nodes, edges, end) + edge_weight(nodes, replacement),
    decreases end,
{
    reveal(edge_total);
    if k < end - 1 {
        edge_update_delta(nodes, edges, k, replacement, (end - 1) as nat);
    } else {
        assert(edges.update(k, replacement).take(end as int - 1) == edges.take(end as int - 1));
        edge_prefix_equal(nodes, edges.update(k, replacement), edges, (end - 1) as nat);
    }
}

proof fn edge_prefix_equal(nodes: Seq<Node>, left: Seq<Dependency>, right: Seq<Dependency>, end: nat)
    requires end <= left.len(), end <= right.len(),
        forall|j: int| 0 <= j < end ==> left[j] == right[j],
    ensures edge_total(nodes, left, end) == edge_total(nodes, right, end),
    decreases end,
{
    reveal(edge_total);
    if end > 0 { edge_prefix_equal(nodes, left, right, (end - 1) as nat); }
}

/// #805: the changed-edge witness plus the frame pays exactly one rank token.
pub(super) proof fn delivery_rank_delta(before: World, after: World, i: int, k: int)
    requires 0 <= i < before.nodes@.len(), i <= u64::MAX as int,
        0 <= k < before.dependencies@.len(),
        dependencies::edges_indexable(before), dependencies::dependency_frame(before, after, i),
        before.dependencies@[k].node == NodeId(i as u64),
        before.dependencies@[k].status == DependencyStatus::Pending,
        !(before.nodes@[i].phase is Closed),
        after.dependencies@ == before.dependencies@.update(k, Dependency {
            status: DependencyStatus::Delivered, rejected: false, ..before.dependencies@[k] }),
    ensures progress_measure(after) + 1 == progress_measure(before),
        dependencies::edges_indexable(after),
{
    delivery_node_frame(before, after, i);
    dependencies::indexable_definition(before);
    dependencies::indexable_definition(after);
    reveal(edge_weight);
    assert forall|j: int| 0 <= j < before.dependencies@.len() implies
        edge_weight(after.nodes@, before.dependencies@[j])
            == edge_weight(before.nodes@, before.dependencies@[j]) by {}
    edge_prefix_frame(before.nodes@, after.nodes@, before.dependencies@, before.dependencies@.len());
    edge_update_delta(after.nodes@, before.dependencies@, k, after.dependencies@[k],
        before.dependencies@.len());
    assert(edge_weight(after.nodes@, before.dependencies@[k]) == 1);
    assert(edge_weight(after.nodes@, after.dependencies@[k]) == 0);
    reveal(progress_measure);
}

/// #805: closure can remove prerequisite debt, and cannot create any.
proof fn edge_total_compare(before: Seq<Node>, after: Seq<Node>, edges: Seq<Dependency>, end: nat)
    requires end <= edges.len(), forall|j: int| 0 <= j < end
        ==> edge_weight(after, edges[j]) <= edge_weight(before, edges[j]),
    ensures edge_total(after, edges, end) <= edge_total(before, edges, end),
    decreases end,
{
    reveal(edge_total);
    if end > 0 { edge_total_compare(before, after, edges, (end - 1) as nat); }
}

/// #805: lift one actual node decrease with the untouched prerequisite table.
pub(super) proof fn lift_node_progress(before: World, after: World, i: int)
    requires 0 <= i < before.nodes@.len(),
        after.nodes@ == before.nodes@.update(i, after.nodes@[i]),
        after.dependencies@ == before.dependencies@, dependencies::edges_indexable(before),
        (before.nodes@[i].phase is Closed ==> after.nodes@[i].phase is Closed),
        distance(after.nodes@[i]) < distance(before.nodes@[i]),
    ensures progress_measure(after) < progress_measure(before),
{
    reveal(progress_measure);
    reveal(edge_weight);
    dependencies::indexable_definition(before);
    sum_update(before.nodes@, i, after.nodes@[i]);
    assert forall|j: int| 0 <= j < before.dependencies@.len() implies
        edge_weight(after.nodes@, before.dependencies@[j]) <= edge_weight(before.nodes@, before.dependencies@[j]) by {
        let owner = before.dependencies@[j].node.0 as int;
        if owner != i { assert(after.nodes@[owner] == before.nodes@[owner]); }
    }
    edge_total_compare(before.nodes@, after.nodes@, before.dependencies@, before.dependencies@.len());
}

/// #805: the real settle relation preserves rows and never reopens an owner.
pub(super) proof fn settle_measure(before: World, after: World)
    requires settle_chain(before, after), dependencies::edges_indexable(before),
        forall|j: int| 0 <= j < before.nodes@.len()
            ==> (before.nodes@[j].phase is Closed ==> after.nodes@[j].phase is Closed),
    ensures progress_measure(after) <= progress_measure(before),
{
    reveal(progress_measure);
    reveal(edge_weight);
    dependencies::indexable_definition(before);
    assert forall|j: int| 0 <= j < before.dependencies@.len() implies
        edge_weight(after.nodes@, before.dependencies@[j]) <= edge_weight(before.nodes@, before.dependencies@[j]) by {}
    edge_total_compare(before.nodes@, after.nodes@, before.dependencies@, before.dependencies@.len());
}
}
