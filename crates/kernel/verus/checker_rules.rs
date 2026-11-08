//! K4a: the complete production checker as one named acceptance predicate.
//! `World::check` returns Ok exactly when `accepted` holds, for every finite
//! world allowed by its existing requires. No size, settledness or fixture bound.
use super::*;

verus! {
/// Every rule of the real World::check: the complete dependency checker
/// (declarations, blocked-owner resources, unique keys and acyclicity), gate and
/// pool configuration, every node rule, disjoint held footprints per repository,
/// per-repository width, pool capacity and unique tickets.
#[verifier::opaque]
pub(crate) open spec fn accepted(w: World) -> bool {
    dependencies::rules_ok(w) && progress_gate_apply::accepted(w)
}

/// The complete acceptance, spelled out for proof consumers.
pub(crate) proof fn accepted_definition(w: World)
    ensures accepted(w) == (dependencies::rules_ok(w)
        && w.gates@.len() > 0 && w.pool_capacity > 0
        && (forall|r: int| 0 <= r < w.gates@.len() ==> w.gates@[r].w > 0
            && progress_gate_apply::holders(w, RepoId(r as u64), w.nodes@.len()) <= w.gates@[r].w)
        && (forall|i: int| 0 <= i < w.nodes@.len() ==> retirement::node_rules(w, w.nodes@[i]))
        && (forall|a: int, b: int| 0 <= a < b < w.nodes@.len() ==> progress_gate_apply::pair(w, a, b))
        && pool_users(w.nodes@, w.nodes@.len()) <= w.pool_capacity
        && (forall|a: int, b: int| 0 <= a < w.nodes@.len() && 0 <= b < w.nodes@.len() && a != b
            ==> (w.nodes@[a].ticket.is_some() ==> w.nodes@[a].ticket != w.nodes@[b].ticket))),
{
    reveal(accepted);
    reveal(progress_gate_apply::accepted);
}

/// Complete acceptance contains the older progress projection and graph indexing.
pub(crate) proof fn accepted_checked(w: World)
    requires accepted(w),
    ensures world_checked(w), dependencies::rules_ok(w), dependencies::edges_indexable(w),
        dependencies::dependencies_checked(w),
{
    reveal(accepted);
    reveal(progress_gate_apply::accepted);
    dependencies::rules_checked(w);
    world_checked_definition(w);
    assert forall|j: int| 0 <= j < w.nodes@.len() implies node_checked(w, w.nodes@[j]) by {
        reveal(retirement::node_rules);
        assert(retirement::node_rules(w, w.nodes@[j]));
    }
}

/// One accepted node satisfies every check_node rule.
pub(crate) proof fn accepted_node(w: World, i: int)
    requires accepted(w), 0 <= i < w.nodes@.len(),
    ensures retirement::node_rules(w, w.nodes@[i]), node_checked(w, w.nodes@[i]),
{
    reveal(accepted);
    reveal(progress_gate_apply::accepted);
    reveal(retirement::node_rules);
}

/// Every issued place in line predates the next ticket and no two nodes share one.
/// This is check_node's `t < next_ticket` rule plus the checker's uniqueness scan.
pub(crate) proof fn tickets_issued(w: World)
    requires accepted(w),
    ensures forall|i: int| 0 <= i < w.nodes@.len() ==> match #[trigger] w.nodes@[i].ticket {
            None => true, Some(t) => t < w.next_ticket,
        },
        forall|a: int, b: int| 0 <= a < w.nodes@.len() && 0 <= b < w.nodes@.len() && a != b
            && (#[trigger] w.nodes@[a]).ticket.is_some()
            ==> w.nodes@[a].ticket != (#[trigger] w.nodes@[b]).ticket,
{
    reveal(accepted);
    reveal(progress_gate_apply::accepted);
    assert forall|i: int| 0 <= i < w.nodes@.len() implies match #[trigger] w.nodes@[i].ticket {
        None => true, Some(t) => t < w.next_ticket,
    } by {
        reveal(retirement::node_rules);
        assert(retirement::node_rules(w, w.nodes@[i]));
    }
}
}
