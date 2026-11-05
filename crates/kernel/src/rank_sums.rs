//! #771/#805: unchanged finite-sum lemmas, independently gated.
use super::*;

verus! {
/// State-table progress measure with a mathematical sum, for any sequence size.
pub(super) open spec fn node_total(nodes: Seq<Node>) -> nat
    decreases nodes.len(),
{
    if nodes.len() == 0 { 0 }
    else { node_total(nodes.drop_last()) + distance(nodes.last()) }
}

/// #771 / #764 Target A: a changed node is one summand at every finite size.
pub(super) proof fn sum_update(nodes: Seq<Node>, index: int, replacement: Node)
    requires 0 <= index < nodes.len(),
    ensures node_total(nodes.update(index, replacement)) + distance(nodes[index])
        == node_total(nodes) + distance(replacement),
    decreases nodes.len(),
{
    if index == nodes.len() - 1 {
        assert(nodes.update(index, replacement).drop_last() == nodes.drop_last());
    } else {
        sum_update(nodes.drop_last(), index, replacement);
        assert(nodes.update(index, replacement).drop_last()
            == nodes.drop_last().update(index, replacement));
        assert(nodes.drop_last()[index] == nodes[index]);
    }
}

/// #771: sum a local allocation potential over arbitrary finite storage.
pub(super) open spec fn potential_prefix(nodes: Seq<Node>, end: nat,
    potential: spec_fn(Node) -> nat) -> nat
    recommends end <= nodes.len(),
    decreases end,
{
    if end == 0 { 0 } else {
        potential_prefix(nodes, (end - 1) as nat, potential)
            + potential(nodes[end as int - 1])
    }
}

pub(super) open spec fn pool_users(nodes: Seq<Node>, end: nat) -> nat {
    potential_prefix(nodes, end, |n: Node| if n.pool { 1nat } else { 0nat })
}

pub(super) open spec fn slot_waiters(nodes: Seq<Node>, end: nat, repo: RepoId) -> nat {
    potential_prefix(nodes, end, |n: Node|
        if n.repo == repo && n.phase is Ready && n.ticket.is_some() { 1nat } else { 0nat })
}

pub(super) proof fn potential_prefix_equal(left: Seq<Node>, right: Seq<Node>, end: nat,
    potential: spec_fn(Node) -> nat)
    requires end <= left.len(), end <= right.len(),
        forall|i: int| 0 <= i < end ==> left[i] == right[i],
    ensures potential_prefix(left, end, potential) == potential_prefix(right, end, potential),
    decreases end,
{
    if end > 0 {
        potential_prefix_equal(left, right, (end - 1) as nat, potential);
    }
}

/// #771: one actual node update changes exactly its allocation potential.
pub(super) proof fn potential_update(nodes: Seq<Node>, index: int,
    replacement: Node, end: nat, potential: spec_fn(Node) -> nat)
    requires 0 <= index < end <= nodes.len(),
    ensures potential_prefix(nodes.update(index, replacement), end, potential)
            + potential(nodes[index])
        == potential_prefix(nodes, end, potential) + potential(replacement),
    decreases end,
{
    if index < end - 1 {
        potential_update(nodes, index, replacement, (end - 1) as nat, potential);
    } else {
        potential_prefix_equal(nodes.update(index, replacement), nodes,
            (end - 1) as nat, potential);
    }
}

/// #771: one node's allocation reserve is contained in the finite sum.
pub(super) proof fn potential_member(nodes: Seq<Node>, index: int, end: nat,
    potential: spec_fn(Node) -> nat)
    requires 0 <= index < end <= nodes.len(),
    ensures potential(nodes[index]) <= potential_prefix(nodes, end, potential),
    decreases end,
{
    if index < end - 1 { potential_member(nodes, index, (end - 1) as nat, potential); }
}

/// Q30/Q39 / #771: every nonempty finite line has an oldest eligible member.
/// The caller supplies the actual ticket or priority/ticket key.
pub(super) proof fn finite_minimum(nodes: Seq<Node>, end: nat, eligible: spec_fn(Node) -> bool,
    key: spec_fn(Node) -> nat)
    requires end <= nodes.len(), exists|i: int| 0 <= i < end && eligible(nodes[i]),
    ensures exists|i: int| 0 <= i < end && eligible(nodes[i])
        && forall|j: int| 0 <= j < end && eligible(nodes[j]) ==> key(nodes[i]) <= key(nodes[j]),
    decreases end,
{
    if exists|i: int| 0 <= i < end - 1 && eligible(nodes[i]) {
        finite_minimum(nodes, (end - 1) as nat, eligible, key);
        let best = choose|i: int| 0 <= i < (end - 1) && eligible(nodes[i])
            && forall|j: int| 0 <= j < (end - 1) && eligible(nodes[j])
                ==> key(nodes[i]) <= key(nodes[j]);
        let last = end - 1;
        if eligible(nodes[last]) && key(nodes[last]) < key(nodes[best]) {
            assert forall|j: int| 0 <= j < end && eligible(nodes[j])
                implies key(nodes[last]) <= key(nodes[j]) by {
                if j < last { assert(key(nodes[best]) <= key(nodes[j])); }
            }
            assert(0 <= last < end && eligible(nodes[last])
                && forall|j: int| 0 <= j < end && eligible(nodes[j]) ==> key(nodes[last]) <= key(nodes[j]));
        } else {
            assert forall|j: int| 0 <= j < end && eligible(nodes[j])
                implies key(nodes[best]) <= key(nodes[j]) by {
                if j < last { assert(key(nodes[best]) <= key(nodes[j])); }
            }
            assert(0 <= best < end && eligible(nodes[best])
                && forall|j: int| 0 <= j < end && eligible(nodes[j]) ==> key(nodes[best]) <= key(nodes[j]));
        }
    } else {
        let last = end - 1;
        assert(eligible(nodes[last]));
        assert forall|j: int| 0 <= j < end && eligible(nodes[j])
            implies key(nodes[last]) <= key(nodes[j]) by { assert(j == last); }
        assert(0 <= last < end && eligible(nodes[last])
            && forall|j: int| 0 <= j < end && eligible(nodes[j]) ==> key(nodes[last]) <= key(nodes[j]));
    }
}

/// #771: the open-node scratch count depends only on the scanned prefix.
pub(super) proof fn open_prefix_equal(left: Seq<Node>, right: Seq<Node>, end: nat)
    requires end <= left.len(), end <= right.len(),
        forall|j: int| 0 <= j < end ==> left[j] == right[j],
    ensures open_prefix(left, end) == open_prefix(right, end),
    decreases end,
{
    reveal(open_prefix);
    if end > 0 { open_prefix_equal(left, right, (end - 1) as nat); }
}

/// #771: a node update cannot reopen any closed history entry during settle.
pub(super) proof fn open_count_update(nodes: Seq<Node>, index: int, replacement: Node, end: nat)
    requires 0 <= index < end <= nodes.len(),
        nodes[index].phase is Closed ==> replacement.phase is Closed,
    ensures open_prefix(nodes.update(index, replacement), end) <= open_prefix(nodes, end),
    decreases end,
{
    reveal(open_prefix);
    if index < end - 1 {
        open_count_update(nodes, index, replacement, (end - 1) as nat);
    } else {
        open_prefix_equal(nodes.update(index, replacement), nodes, (end - 1) as nat);
    }
}

/// #771: compose the real local postcondition with arbitrary finite sums.
pub(super) proof fn advance_sum_bounds(before: Seq<Node>, after: Seq<Node>, index: int,
    ticket_before: u64, ticket_after: u64, changed: bool)
    requires 0 <= index < before.len(), after == before.update(index, after[index]),
        advance_post(before[index], after[index], ticket_before, ticket_after, changed),
    ensures node_total(after) <= node_total(before),
        potential_prefix(after, after.len(), |n: Node| internal_height(n))
            <= potential_prefix(before, before.len(), |n: Node| internal_height(n)),
        changed ==> potential_prefix(after, after.len(), |n: Node| internal_height(n))
            < potential_prefix(before, before.len(), |n: Node| internal_height(n)),
        ticket_after as int + potential_prefix(after, after.len(), |n: Node| ticket_capacity(n))
            <= ticket_before as int + potential_prefix(before, before.len(), |n: Node| ticket_capacity(n)),
        open_prefix(after, after.len()) <= open_prefix(before, before.len()),
{
    reveal(advance_post);
    sum_update(before, index, after[index]);
    potential_update(before, index, after[index], before.len(), |n: Node| internal_height(n));
    potential_update(before, index, after[index], before.len(), |n: Node| ticket_capacity(n));
    if before[index].phase is Closed {
        assert(internal_height(before[index]) == 0);
        assert(!changed);
    }
    open_count_update(before, index, after[index], before.len());
}

/// #771: closed history contributes no scratch or allocation reserve.
/// The bound depends on open nodes, not the total length of executable storage.
pub(super) proof fn potential_bound(nodes: Seq<Node>, end: nat,
    potential: spec_fn(Node) -> nat, per_open: nat)
    requires end <= nodes.len(),
        forall|i: int| 0 <= i < end ==> potential(nodes[i]) <= per_open
            && (nodes[i].phase is Closed ==> potential(nodes[i]) == 0),
    ensures potential_prefix(nodes, end, potential)
        <= per_open * open_prefix(nodes, end),
    decreases end,
{
    reveal(open_prefix);
    if end > 0 {
        potential_bound(nodes, (end - 1) as nat, potential, per_open);
        let prefix = potential_prefix(nodes, (end - 1) as nat, potential);
        let open = open_prefix(nodes, (end - 1) as nat);
        let value = potential(nodes[end as int - 1]);
        assert(prefix <= per_open * open);
        assert(value <= per_open);
        if nodes[end as int - 1].phase is Closed {
            assert(value == 0);
        } else {
            assert(prefix <= per_open * open && value <= per_open
                ==> prefix + value <= per_open * (open + 1)) by(nonlinear_arith);
        }
    }
}

/// #771: the real local settle contracts consume these finite-world reserves.
pub(super) proof fn settle_reserve_totals(nodes: Seq<Node>)
    ensures
        potential_prefix(nodes, nodes.len(), |n: Node| internal_height(n)) <= 6 * open_prefix(nodes, nodes.len()),
        potential_prefix(nodes, nodes.len(), |n: Node| generation_capacity(n)) <= open_prefix(nodes, nodes.len()),
        potential_prefix(nodes, nodes.len(), |n: Node| effect_capacity(n)) <= open_prefix(nodes, nodes.len()),
        potential_prefix(nodes, nodes.len(), |n: Node| ticket_capacity(n)) <= 2 * open_prefix(nodes, nodes.len()),
        potential_prefix(nodes, nodes.len(), |n: Node| merge_capacity(n)) <= open_prefix(nodes, nodes.len()),
{
    assert forall|i: int| 0 <= i < nodes.len() implies
        internal_height(nodes[i]) <= 6
        && generation_capacity(nodes[i]) <= 1
        && effect_capacity(nodes[i]) <= 1
        && ticket_capacity(nodes[i]) <= 2
        && merge_capacity(nodes[i]) <= 1
        && (nodes[i].phase is Closed ==> internal_height(nodes[i]) == 0
            && generation_capacity(nodes[i]) == 0 && effect_capacity(nodes[i]) == 0
            && ticket_capacity(nodes[i]) == 0 && merge_capacity(nodes[i]) == 0)
    by { local_capacities(nodes[i]); }
    potential_bound(nodes, nodes.len(), |n: Node| internal_height(n), 6);
    potential_bound(nodes, nodes.len(), |n: Node| generation_capacity(n), 1);
    potential_bound(nodes, nodes.len(), |n: Node| effect_capacity(n), 1);
    potential_bound(nodes, nodes.len(), |n: Node| ticket_capacity(n), 2);
    potential_bound(nodes, nodes.len(), |n: Node| merge_capacity(n), 1);
}
}
