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
/// grant needs room in the width and both orientations of disjointness from
/// every other holder: the scheduler's slot_spec gives one orientation and
/// footprint symmetry the other, at the consumer.
pub(super) open spec fn step_premises(before: World, after: World, i: int) -> bool {
    0 <= i < before.nodes@.len()
        && after.nodes@ == before.nodes@.update(i, after.nodes@[i])
        && after.gates@ == before.gates@ && after.file_sets@ == before.file_sets@
        && after.dependencies@ == before.dependencies@
        && after.pool_capacity == before.pool_capacity
        && after.next_question == before.next_question
        && before.next_ticket <= after.next_ticket
        && before.gates@.len() <= u64::MAX
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
                    && footprint::disjoint(before, me.footprint, before.nodes@[j].footprint)
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
    ensures checking::accepted(after),
{
    checking::accepted_parts(before);
    dependencies::node_update_preserves(before, after, i);
    step_preserves_accepted(before, after, i);
}
}
