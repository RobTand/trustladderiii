//! K4c: complete acceptance across one internal transition of one node.
//! The world-level argument is proved here for arbitrary finite worlds. Its
//! node-level premises are explicit requires that each advance arm and the
//! scheduler's grant queries establish, so nothing about advance is assumed.
use super::*;

verus! {
/// The scheduler's slot count without node `skip`, plus that node's own slot,
/// is the checker's holder count.
pub(super) proof fn other_holders_bridge(w: World, end: nat, repo: RepoId, skip: int)
    requires end <= w.nodes@.len(),
    ensures scheduler::other_holders(w.nodes@, end, repo, skip)
            + (if 0 <= skip < end && w.nodes@[skip].holds && w.nodes@[skip].repo == repo { 1nat } else { 0nat })
        == progress_gate_apply::holders(w, repo, end),
    decreases end,
{
    if end > 0 { other_holders_bridge(w, (end - 1) as nat, repo, skip); }
}

/// The checker's holder count after node i alone changes.
pub(super) proof fn holders_update(before: World, after: World, i: int, repo: RepoId, end: nat)
    requires 0 <= i < before.nodes@.len(), end <= before.nodes@.len(),
        after.nodes@ == before.nodes@.update(i, after.nodes@[i]),
    ensures progress_gate_apply::holders(after, repo, end)
            + (if i < end && before.nodes@[i].holds && before.nodes@[i].repo == repo { 1nat } else { 0nat })
        == progress_gate_apply::holders(before, repo, end)
            + (if i < end && after.nodes@[i].holds && after.nodes@[i].repo == repo { 1nat } else { 0nat }),
    decreases end,
{
    if end > 0 {
        holders_update(before, after, i, repo, (end - 1) as nat);
        if end - 1 != i { assert(after.nodes@[end - 1] == before.nodes@[end - 1]); }
    }
}

/// The pool occupancy after node i alone changes.
pub(super) proof fn pool_update(before: Seq<Node>, after: Seq<Node>, i: int, end: nat)
    requires 0 <= i < before.len(), end <= before.len(), after == before.update(i, after[i]),
    ensures pool_users(after, end) + (if i < end && before[i].pool { 1nat } else { 0nat })
        == pool_users(before, end) + (if i < end && after[i].pool { 1nat } else { 0nat }),
    decreases end,
{
    if end > 0 {
        pool_update(before, after, i, (end - 1) as nat);
        if end - 1 != i { assert(after[end - 1] == before[end - 1]); }
    }
}

/// Node rules read next_ticket only as an upper bound on issued tickets.
pub(super) proof fn node_rules_ticket_grows(before: World, after: World, m: Node)
    requires retirement::node_rules(before, m), before.gates@ == after.gates@,
        before.file_sets@ == after.file_sets@, before.next_question == after.next_question,
        before.next_ticket <= after.next_ticket,
    ensures retirement::node_rules(after, m),
{
    reveal(retirement::node_rules);
    footprint::storage_frame(before, after, m.footprint, m.footprint);
}

/// The node-level facts one internal step of node i must establish. A slot
/// grant needs room in the width and disjointness from every other holder,
/// in the orientation the scheduler's slot_spec proves; footprint symmetry
/// gives the checker's other orientation here.
pub(super) open spec fn step_premises(before: World, after: World, i: int) -> bool {
    0 <= i < before.nodes@.len()
        && after.nodes@ == before.nodes@.update(i, after.nodes@[i])
        && after.gates@ == before.gates@ && after.file_sets@ == before.file_sets@
        && after.dependencies@ == before.dependencies@
        && after.pool_capacity == before.pool_capacity
        && after.next_question == before.next_question
        && before.next_ticket <= after.next_ticket
        && before.gates@.len() <= u64::MAX
        && footprint::ordered_table(before)
        && after.nodes@[i].repo == before.nodes@[i].repo
        && after.nodes@[i].footprint == before.nodes@[i].footprint
        && retirement::node_rules(after, after.nodes@[i])
        && (after.nodes@[i].holds && !before.nodes@[i].holds ==> {
            let me = before.nodes@[i];
            &&& me.repo.0 < before.gates@.len()
            &&& scheduler::other_holders(before.nodes@, before.nodes@.len(), me.repo, i)
                < before.gates@[me.repo.0 as int].w
            &&& forall|j: int| 0 <= j < before.nodes@.len() && j != i && before.nodes@[j].holds
                && before.nodes@[j].repo == me.repo
                ==> footprint::disjoint(before, before.nodes@[j].footprint, me.footprint)
        })
        && (after.nodes@[i].pool && !before.nodes@[i].pool
            ==> pool_users(before.nodes@, before.nodes@.len()) < before.pool_capacity)
        && (after.nodes@[i].ticket == before.nodes@[i].ticket || after.nodes@[i].ticket.is_none()
            || (before.nodes@[i].ticket.is_none() && after.nodes@[i].ticket == Some(before.next_ticket)))
}

/// Every non-dependency checker rule survives one internal step of node i.
pub(super) proof fn step_preserves_gate_rules(before: World, after: World, i: int)
    requires checking::accepted(before), step_premises(before, after, i),
    ensures progress_gate_apply::accepted(after),
{
    checking::accepted_parts(before);
    checking::tickets_issued(before);
    reveal(progress_gate_apply::accepted);
    let bn = before.nodes@[i];
    let an = after.nodes@[i];
    let len = before.nodes@.len();
    assert(after.nodes@.len() == len);
    assert forall|j: int| 0 <= j < len
        implies retirement::node_rules(after, #[trigger] after.nodes@[j]) by {
        if j != i {
            assert(after.nodes@[j] == before.nodes@[j]);
            node_rules_ticket_grows(before, after, before.nodes@[j]);
        }
    }
    assert forall|x: int, y: int| 0 <= x < y < len
        implies progress_gate_apply::pair(after, x, y) by {
        footprint::storage_frame(before, after, before.nodes@[x].footprint, before.nodes@[y].footprint);
        assert(progress_gate_apply::pair(before, x, y));
        if x != i && y != i {
            assert(after.nodes@[x] == before.nodes@[x] && after.nodes@[y] == before.nodes@[y]);
        } else if x == i {
            assert(after.nodes@[y] == before.nodes@[y]);
            if an.holds && !bn.holds && after.nodes@[y].holds && after.nodes@[y].repo == an.repo {
                checking::accepted_node(before, i);
                checking::accepted_node(before, y);
                footprint::disjoint_symmetric(before, before.nodes@[y].footprint, bn.footprint);
            }
        } else {
            assert(after.nodes@[x] == before.nodes@[x]);
        }
    }
    assert forall|r: int| 0 <= r < after.gates@.len()
        implies after.gates@[r].w > 0
            && progress_gate_apply::holders(after, RepoId(r as u64), len) <= after.gates@[r].w by {
        let repo = RepoId(r as u64);
        assert(progress_gate_apply::holders(before, repo, len) <= before.gates@[r].w);
        holders_update(before, after, i, repo, len);
        if an.holds && !bn.holds && an.repo == repo {
            other_holders_bridge(before, len, repo, i);
            assert(bn.repo.0 as int == r);
        }
    }
    pool_update(before.nodes@, after.nodes@, i, len);
    assert forall|x: int, y: int| 0 <= x < len && 0 <= y < len && x != y
        implies (after.nodes@[x].ticket.is_some() ==> after.nodes@[x].ticket != after.nodes@[y].ticket) by {
        if x != i && y != i {
            assert(after.nodes@[x] == before.nodes@[x] && after.nodes@[y] == before.nodes@[y]);
        } else if x == i {
            assert(after.nodes@[y] == before.nodes@[y]);
            assert(before.nodes@[y].ticket.is_some() ==> before.nodes@[y].ticket.unwrap() < before.next_ticket);
        } else {
            assert(after.nodes@[x] == before.nodes@[x]);
            assert(before.nodes@[x].ticket.is_some() ==> before.nodes@[x].ticket.unwrap() < before.next_ticket);
        }
    }
}

/// Complete acceptance survives one internal step of node i, given the
/// dependency rules after the step (the checker's endpoint-update export).
pub(super) proof fn step_preserves_accepted(before: World, after: World, i: int)
    requires checking::accepted(before), step_premises(before, after, i),
        dependencies::rules_ok(after),
    ensures checking::accepted(after),
{
    step_preserves_gate_rules(before, after, i);
    checking::accepted_parts(after);
}

/// Complete acceptance across one internal step of node i. The dependency
/// rules come from the checker's node-update export: the stepped owner either
/// has every prerequisite delivered (Admit included) or still meets every
/// rule on a waiting owner.
pub(super) proof fn step_preserves_accepted_full(before: World, after: World, i: int)
    requires checking::accepted(before), step_premises(before, after, i),
        before.nodes@.len() <= u64::MAX,
        before.nodes@[i].phase is Closed ==> after.nodes@[i].phase is Closed,
        after.nodes@[i].credit.withheld ==> before.nodes@[i].credit.withheld
            || !dependencies::prerequisites_delivered(before, NodeId(i as u64)),
        dependencies::prerequisites_delivered(before, NodeId(i as u64))
            || dependencies::owner_waiting_ok(after.nodes@[i]),
    ensures checking::accepted(after),
{
    checking::accepted_parts(before);
    dependencies::node_update_preserves(before, after, i);
    step_preserves_accepted(before, after, i);
}

// K5 G19: complete acceptance through apply. One event changes node i (C1) or
// appends one node (C2). The node-level facts are explicit premises N1-N5,
// which each handler family exports and Apply's dispatcher discharges.

/// The file table only grows at the end.
pub(super) open spec fn table_grows(before: World, after: World) -> bool {
    after.file_sets@.len() >= before.file_sets@.len()
        && forall|j: int| 0 <= j < before.file_sets@.len()
            ==> #[trigger] after.file_sets@[j] == before.file_sets@[j]
}

/// N1: node i meets every node rule after the event.
pub(super) open spec fn apply_n1_rules(after: World, i: int) -> bool {
    retirement::node_rules(after, after.nodes@[i])
}

/// N2: an event grants nothing: slot and pool are kept or released, and the
/// place in line is kept or cleared.
pub(super) open spec fn apply_n2_no_grants(b: Node, n: Node) -> bool {
    (n.holds ==> b.holds) && (n.pool ==> b.pool) && (n.ticket == b.ticket || n.ticket.is_none())
}

/// N3: same repository, and either the same footprint or, while node i holds,
/// a new footprint disjoint in both orientations from every other holder in
/// that repository (MakerNeedsFiles).
pub(super) open spec fn apply_n3_footprint(after: World, b: Node, i: int) -> bool {
    let n = after.nodes@[i];
    n.repo == b.repo && (n.footprint == b.footprint || (n.holds ==> forall|j: int|
        0 <= j < after.nodes@.len() && j != i && (#[trigger] after.nodes@[j]).holds
            && after.nodes@[j].repo == n.repo
        ==> footprint::disjoint(after, n.footprint, after.nodes@[j].footprint)
            && footprint::disjoint(after, after.nodes@[j].footprint, n.footprint)))
}

/// N4: the dependency table is unchanged and node i meets the checker's
/// node-update premises (dependencies::node_update_preserves).
pub(super) open spec fn apply_n4_dependencies(before: World, after: World, i: int) -> bool {
    after.dependencies@ == before.dependencies@
        && before.nodes@.len() <= u64::MAX
        && (before.nodes@[i].phase is Closed ==> after.nodes@[i].phase is Closed)
        && (after.nodes@[i].credit.withheld ==> before.nodes@[i].credit.withheld
            || !dependencies::prerequisites_delivered(before, NodeId(i as u64)))
        && (dependencies::prerequisites_delivered(before, NodeId(i as u64))
            || dependencies::owner_waiting_ok(after.nodes@[i]))
}

/// N5: the world frame of one event on node i. Other nodes, gates and pool
/// capacity are unchanged; the file table, next_ticket and next_question only grow.
pub(super) open spec fn apply_n5_frame(before: World, after: World, i: int) -> bool {
    0 <= i < before.nodes@.len()
        && after.nodes@ == before.nodes@.update(i, after.nodes@[i])
        && after.gates@ == before.gates@ && after.pool_capacity == before.pool_capacity
        && table_grows(before, after)
        && before.next_ticket <= after.next_ticket && before.next_question <= after.next_question
}

/// Every non-dependency checker rule survives one event on node i.
pub(super) proof fn apply_preserves_gate_rules(before: World, after: World, i: int)
    requires checking::accepted(before), apply_n5_frame(before, after, i), apply_n1_rules(after, i),
        apply_n2_no_grants(before.nodes@[i], after.nodes@[i]),
        apply_n3_footprint(after, before.nodes@[i], i),
    ensures progress_gate_apply::accepted(after),
{
    checking::accepted_parts(before);
    reveal(progress_gate_apply::accepted);
    let bn = before.nodes@[i];
    let an = after.nodes@[i];
    let len = before.nodes@.len();
    assert(after.nodes@.len() == len);
    assert forall|j: int| 0 <= j < len
        implies retirement::node_rules(after, #[trigger] after.nodes@[j]) by {
        if j != i {
            assert(after.nodes@[j] == before.nodes@[j]);
            footprint::node_rules_grow(before, after, before.nodes@[j]);
        }
    }
    assert forall|x: int, y: int| 0 <= x < y < len
        implies progress_gate_apply::pair(after, x, y) by {
        if after.nodes@[x].holds && after.nodes@[y].holds && after.nodes@[x].repo == after.nodes@[y].repo {
            checking::accepted_node(before, x);
            checking::accepted_node(before, y);
            assert(progress_gate_apply::pair(before, x, y));
            if (x != i && y != i) || an.footprint == bn.footprint {
                if x != i { assert(after.nodes@[x] == before.nodes@[x]); }
                if y != i { assert(after.nodes@[y] == before.nodes@[y]); }
                footprint::disjoint_under_table_growth(before, after,
                    before.nodes@[x].footprint, before.nodes@[y].footprint);
            } else if x == i {
                assert(after.nodes@[y].holds);
            } else {
                assert(after.nodes@[x].holds);
            }
        }
    }
    assert forall|r: int| 0 <= r < after.gates@.len()
        implies after.gates@[r].w > 0
            && progress_gate_apply::holders(after, RepoId(r as u64), len) <= after.gates@[r].w by {
        let repo = RepoId(r as u64);
        assert(progress_gate_apply::holders(before, repo, len) <= before.gates@[r].w);
        holders_update(before, after, i, repo, len);
    }
    pool_update(before.nodes@, after.nodes@, i, len);
    assert forall|x: int, y: int| 0 <= x < len && 0 <= y < len && x != y
        implies (after.nodes@[x].ticket.is_some() ==> after.nodes@[x].ticket != after.nodes@[y].ticket) by {
        if x != i && y != i {
            assert(after.nodes@[x] == before.nodes@[x] && after.nodes@[y] == before.nodes@[y]);
        } else if x == i {
            assert(after.nodes@[y] == before.nodes@[y]);
        } else {
            assert(after.nodes@[x] == before.nodes@[x]);
        }
    }
}

/// C1: complete acceptance across one event on node i, given the dependency
/// rules after it (Dependency / DependencyRemove: the checker's fact_preserves
/// and remove_preserves).
pub(super) proof fn apply_step_preserves_accepted(before: World, after: World, i: int)
    requires checking::accepted(before), apply_n5_frame(before, after, i), apply_n1_rules(after, i),
        apply_n2_no_grants(before.nodes@[i], after.nodes@[i]),
        apply_n3_footprint(after, before.nodes@[i], i),
        dependencies::rules_ok(after),
    ensures checking::accepted(after),
{
    apply_preserves_gate_rules(before, after, i);
    checking::accepted_parts(after);
}

/// C1 for every family that leaves the dependency table alone: the dependency
/// rules come from the checker's node-update export (N4).
pub(super) proof fn apply_step_preserves_accepted_full(before: World, after: World, i: int)
    requires checking::accepted(before), apply_n5_frame(before, after, i),
        apply_n2_no_grants(before.nodes@[i], after.nodes@[i]),
        apply_n3_footprint(after, before.nodes@[i], i),
        apply_n4_dependencies(before, after, i),
    ensures checking::accepted(after),
{
    checking::accepted_parts(before);
    dependencies::node_update_preserves(before, after, i);
    apply_step_preserves_accepted(before, after, i);
}

/// The world frame of NewRequest / NewAdoption: one node appended, nothing
/// else changed except the growing file table and counters.
pub(super) open spec fn append_frame(before: World, after: World) -> bool {
    after.nodes@.len() == before.nodes@.len() + 1
        && (forall|j: int| 0 <= j < before.nodes@.len() ==> #[trigger] after.nodes@[j] == before.nodes@[j])
        && after.gates@ == before.gates@ && after.pool_capacity == before.pool_capacity
        && after.dependencies@ == before.dependencies@
        && table_grows(before, after)
        && before.next_ticket <= after.next_ticket && before.next_question <= after.next_question
}

/// The appended node meets the node rules and holds nothing: no slot, no pool
/// run, no place in line and no withheld credit.
pub(super) open spec fn appended_node_ok(after: World, n: Node) -> bool {
    retirement::node_rules(after, n) && !n.holds && !n.pool && n.ticket.is_none() && !n.credit.withheld
}

/// Holder and pool counts over an unchanged prefix.
pub(super) proof fn prefix_counts(before: World, after: World, repo: RepoId, end: nat)
    requires end <= before.nodes@.len(), end <= after.nodes@.len(),
        forall|j: int| 0 <= j < end ==> #[trigger] after.nodes@[j] == before.nodes@[j],
    ensures progress_gate_apply::holders(after, repo, end) == progress_gate_apply::holders(before, repo, end),
        pool_users(after.nodes@, end) == pool_users(before.nodes@, end),
    decreases end,
{
    if end > 0 {
        prefix_counts(before, after, repo, (end - 1) as nat);
        assert(after.nodes@[end - 1] == before.nodes@[end - 1]);
    }
}

/// C2: complete acceptance across NewRequest / NewAdoption.
pub(super) proof fn append_preserves_accepted(before: World, after: World)
    requires checking::accepted(before), append_frame(before, after),
        appended_node_ok(after, after.nodes@[before.nodes@.len() as int]),
    ensures checking::accepted(after),
{
    checking::accepted_parts(before);
    reveal(progress_gate_apply::accepted);
    let len = before.nodes@.len();
    assert forall|j: int| 0 <= j < len
        implies dependencies::same_endpoint(#[trigger] before.nodes@[j], after.nodes@[j]) by {
        assert(after.nodes@[j] == before.nodes@[j]);
    }
    assert forall|j: int| len <= j < after.nodes@.len()
        implies !(#[trigger] after.nodes@[j]).credit.withheld by {
        assert(j == len);
    }
    dependencies::append_preserves(before, after);
    assert forall|j: int| 0 <= j < after.nodes@.len()
        implies retirement::node_rules(after, #[trigger] after.nodes@[j]) by {
        if j < len {
            assert(after.nodes@[j] == before.nodes@[j]);
            footprint::node_rules_grow(before, after, before.nodes@[j]);
        }
    }
    assert forall|x: int, y: int| 0 <= x < y < after.nodes@.len()
        implies progress_gate_apply::pair(after, x, y) by {
        if y < len {
            assert(after.nodes@[x] == before.nodes@[x] && after.nodes@[y] == before.nodes@[y]);
            if after.nodes@[x].holds && after.nodes@[y].holds && after.nodes@[x].repo == after.nodes@[y].repo {
                checking::accepted_node(before, x);
                checking::accepted_node(before, y);
                assert(progress_gate_apply::pair(before, x, y));
                footprint::disjoint_under_table_growth(before, after,
                    before.nodes@[x].footprint, before.nodes@[y].footprint);
            }
        } else {
            assert(y == len);
        }
    }
    assert forall|r: int| 0 <= r < after.gates@.len()
        implies after.gates@[r].w > 0
            && progress_gate_apply::holders(after, RepoId(r as u64), after.nodes@.len()) <= after.gates@[r].w by {
        let repo = RepoId(r as u64);
        prefix_counts(before, after, repo, len);
        assert(progress_gate_apply::holders(before, repo, len) <= before.gates@[r].w);
        assert(progress_gate_apply::holders(after, repo, len + 1) == progress_gate_apply::holders(after, repo, len));
    }
    prefix_counts(before, after, RepoId(0), len);
    assert(pool_users(after.nodes@, len + 1) == pool_users(after.nodes@, len));
    assert forall|x: int, y: int| 0 <= x < after.nodes@.len() && 0 <= y < after.nodes@.len() && x != y
        implies (after.nodes@[x].ticket.is_some() ==> after.nodes@[x].ticket != after.nodes@[y].ticket) by {
        if x < len && y < len {
            assert(after.nodes@[x] == before.nodes@[x] && after.nodes@[y] == before.nodes@[y]);
        } else if x < len {
            assert(y == len);
        } else {
            assert(x == len);
        }
    }
    checking::accepted_parts(after);
}
}
