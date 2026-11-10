//! Finite reserves for the original four-leaf internal trace.
use super::*;

verus! {
#[verifier::opaque]
pub(super) open spec fn small(n: Node, k: int) -> bool {
    n.generation <= k && n.next_seq <= k && n.credit.start <= k
        && n.credit.earned <= k && n.credit.added <= k && n.credit.spent <= k
}

#[verifier::opaque]
pub(super) open spec fn context(w: World, k: int) -> bool {
    checking::accepted(w) && footprint::ordered_table(w)
        && w.nodes@.len() == 4 && w.gates@.len() == 1 && w.dependencies@.len() == 0
        && w.next_ticket <= k && w.next_question <= k
        && forall|j: int| 0 <= j < 4 ==> small(w.nodes@[j], k)
}

pub(super) proof fn reserves(w: World, k: int, i: int)
    requires context(w, k), 0 <= k <= 1000, 0 <= i < 4,
    ensures scheduling_safe(w), node_headroom(w.nodes@[i]),
        arithmetic_consistent(w.nodes@[i]), advance_pre(w.nodes@[i], w.next_ticket, 0),
        world_headroom(w), storage_ordered(w), w.next_question < u64::MAX,
{
    reveal(context); reveal(small); reveal(world_headroom); reveal(storage_ordered);
    checking::accepted_checked(w);
    checking::accepted_node(w, i);
    open_prefix_monotone(w.nodes@, 0, 4);
    reveal_with_fuel(open_prefix, 1);
    assert forall|j: int| 0 <= j < 4 implies node_headroom(w.nodes@[j]) by {}
    assert forall|j: int| 0 <= j < 4 implies
        footprint::valid_footprint(w, w.nodes@[j].footprint)
        && ((w.nodes@[j].holds || w.nodes@[j].pool) ==> !(w.nodes@[j].phase is Closed)) by {
        checking::accepted_node(w, j);
    }
    ranks::local_capacities(w.nodes@[i]);
}

pub(super) proof fn advance_bounds(b: Node, n: Node, t0: u64, t1: u64, moved: bool, k: int)
    requires small(b, k), advance_post(b, n, t0, t1, moved), t0 <= k,
    ensures small(n, k + 2), t1 <= k + 2,
{
    reveal(small); reveal(advance_post);
    ranks::local_capacities(b);
}

pub(super) proof fn advance_accepts(b: World, n: World, i: int)
    requires checking::accepted(b), scheduling_safe(b), b.nodes@.len() == 4,
        b.gates@.len() == 1, 0 <= i < 4,
        advance_family_post::step_rows(b, n, i),
        n.nodes@ == b.nodes@.update(i, n.nodes@[i]),
        n.gates@ == b.gates@, n.file_sets@ == b.file_sets@,
        n.dependencies@ == b.dependencies@, n.pool_capacity == b.pool_capacity,
        n.next_question == b.next_question, b.next_ticket <= n.next_ticket,
        !b.nodes@[i].holds && n.nodes@[i].holds ==> slot_spec(b, i),
        !b.nodes@[i].pool && n.nodes@[i].pool ==> pool_users(b.nodes@, 4) < b.pool_capacity,
    ensures checking::accepted(n),
{
    advance_family_frames::step_premises_lift(b, n, i);
    internal_safety::step_preserves_accepted_full(b, n, i);
}

pub(super) proof fn event_accepts(b: World, n: World, i: int)
    requires checking::accepted(b), b.nodes@.len() == 4, b.dependencies@.len() == 0,
        0 <= i < 4, internal_safety::apply_n5_frame(b, n, i),
        retirement::node_rules(n, n.nodes@[i]),
        internal_safety::apply_n2_no_grants(b.nodes@[i], n.nodes@[i]),
        n.nodes@[i].repo == b.nodes@[i].repo, n.nodes@[i].footprint == b.nodes@[i].footprint,
        n.dependencies@ == b.dependencies@,
        !(b.nodes@[i].phase is Closed),
        !n.nodes@[i].credit.withheld,
    ensures checking::accepted(n),
{
    dependencies::empty_delivered(b, NodeId(i as u64));
    dependencies::prerequisite_definition(b, NodeId(i as u64));
    internal_safety::apply_step_preserves_accepted_full(b, n, i);
}

pub(super) proof fn context_frame(b: World, n: World, i: int, k: int, extra: int)
    requires context(b, k), extra >= 0, 0 <= i < 4, checking::accepted(n),
        n.nodes@ == b.nodes@.update(i, n.nodes@[i]), n.gates@ == b.gates@,
        n.file_sets@ == b.file_sets@, n.dependencies@ == b.dependencies@,
        n.next_ticket <= k + extra, n.next_question <= k + extra,
        small(n.nodes@[i], k + extra),
    ensures context(n, k + extra),
{
    reveal(context); reveal(footprint::ordered_table);
    assert forall|j: int| 0 <= j < 4 implies small(n.nodes@[j], k + extra) by {
        if j != i { reveal(small); }
    }
}
}
