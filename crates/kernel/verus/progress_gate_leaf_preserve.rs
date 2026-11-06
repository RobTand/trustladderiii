//! Q48 / #806: local checker and holder-pair proofs for one real leaf step.
use super::progress_gate_leaf_state::*;
use super::*;

verus! {
/// Q48: prove the changed node against the full local checker separately from
/// pair exclusion and finite capacity arithmetic.
pub(super) proof fn update_node(before: World, after: World, i: int)
    requires domain(before), retirement_wide::accepted(before), low(before), 0 <= i < 4,
        tracked(before.nodes@[i]), before.nodes@[i].ticket.is_some() || !(before.nodes@[i].phase is Ready),
        frame(before, after, i), after.nodes@[i] == updated(before, i),
    ensures forall|j: int| 0 <= j < 4 ==> retirement::node_rules(after, after.nodes@[j]),
{
    reveal(updated); reveal(retirement_wide::accepted); reveal(retirement::node_rules);
    reveal(retirement_gate::slot_node);
    assert forall|j: int| 0 <= j < 4 implies retirement::node_rules(after, after.nodes@[j]) by {
        retirement_frames::node_frame(before, after, before.nodes@[j]);
        footprint::storage_frame(before, after, before.nodes@[j].footprint, before.nodes@[j].footprint);
    }
}

/// Q48: granting the changed leaf preserves each original holder-pair exclusion.
pub(super) proof fn update_pairs(before: World, after: World, i: int)
    requires domain(before), retirement_wide::accepted(before), low(before), 0 <= i < 4,
        tracked(before.nodes@[i]), before.nodes@[i].ticket.is_some() || !(before.nodes@[i].phase is Ready),
        frame(before, after, i), after.nodes@[i] == updated(before, i),
    ensures forall|a: int, b: int| 0 <= a < b < 4 ==> retirement_wide::pair(after, a, b),
{
    reveal(updated); reveal(retirement_wide::accepted); reveal(retirement::node_rules);
    reveal(retirement_gate::slot_node);
    assert forall|a: int, b: int| 0 <= a < b < 4 implies retirement_wide::pair(after, a, b) by {
        footprint::storage_frame(before, after, before.nodes@[a].footprint, before.nodes@[b].footprint);
        assert(retirement_wide::pair(before, a, b));
        assert(after.nodes@[a].footprint == before.nodes@[a].footprint);
        assert(after.nodes@[b].footprint == before.nodes@[b].footprint);
        if after.nodes@[i].holds && !before.nodes@[i].holds {
            assert(retirement_gate::slot(before, i));
            if a == i && after.nodes@[b].holds {
                assert(footprint::disjoint(before, before.nodes@[b].footprint, before.nodes@[i].footprint));
                footprint::inline_disjoint_symmetric(before, before.nodes@[i].footprint, before.nodes@[b].footprint);
            }
            if b == i && after.nodes@[a].holds {
                assert(footprint::disjoint(before, before.nodes@[a].footprint, before.nodes@[i].footprint));
            }
        } else { assert(retirement_wide::pair(before, a, b)); }
    }
}

/// Q48 / #806: only a successful real slot scan can add a holder; cleanup cannot
/// add a pool user. Prove these finite sums without the node-checker context.
pub(super) proof fn update_capacity(before: World, after: World, i: int)
    requires domain(before), retirement_wide::accepted(before), low(before), 0 <= i < 4,
        tracked(before.nodes@[i]), before.nodes@[i].ticket.is_some() || !(before.nodes@[i].phase is Ready),
        frame(before, after, i), after.nodes@[i] == updated(before, i),
    ensures domain(after), after.gates@.len() > 0, after.pool_capacity > 0,
        forall|r: int| 0 <= r < after.gates@.len() ==> after.gates@[r].w > 0
            && retirement_wide::holders(after, RepoId(r as u64), 4) <= after.gates@[r].w,
        pool_users(after.nodes@, 4) <= after.pool_capacity,
{
    reveal(updated); reveal(retirement_wide::accepted); reveal(retirement_gate::slot_node);
    reveal_with_fuel(potential_prefix, 5); reveal_with_fuel(retirement_gate::slot_count, 5);
}

/// Q48 / #806: real tracked steps retain or clear existing tickets, so none can
/// introduce a duplicate ticket. No fresh ticket is allocated by this sequence.
pub(super) proof fn update_tickets(before: World, after: World, i: int)
    requires domain(before), retirement_wide::accepted(before), low(before), 0 <= i < 4,
        tracked(before.nodes@[i]), before.nodes@[i].ticket.is_some() || !(before.nodes@[i].phase is Ready),
        frame(before, after, i), after.nodes@[i] == updated(before, i),
    ensures forall|a: int, b: int| 0 <= a < 4 && 0 <= b < 4 && a != b ==>
        (after.nodes@[a].ticket.is_some() ==> after.nodes@[a].ticket != after.nodes@[b].ticket),
{
    reveal(updated); reveal(retirement_wide::accepted); reveal(retirement_gate::slot_node);
}

}
