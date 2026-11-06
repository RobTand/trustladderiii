//! Q30/Q39 / #806: full gate consistency and apply progress premises.
use super::*;

verus! {
/// #806: original gate storage, also admitting smaller checker-only worlds.
/// Empty prerequisites and no withheld credit follow from gate_world's constructor.
pub(super) open spec fn domain(w: World) -> bool {
    w.nodes@.len() <= 4 && w.gates@.len() > 0 && w.dependencies@.len() == 0
        && forall|i: int| 0 <= i < w.nodes@.len() ==> !w.nodes@[i].credit.withheld
}

/// Q30: exact prefix count in the production consistency checker.
pub(super) open spec fn holders(w: World, repo: RepoId, end: nat) -> nat {
    potential_prefix(w.nodes@, end, |n: Node| if n.holds && n.repo == repo { 1nat } else { 0nat })
}

/// Q30: production checker orientation of each pair of held footprints.
pub(super) open spec fn pair(w: World, a: int, b: int) -> bool {
    w.nodes@[a].holds && w.nodes@[b].holds && w.nodes@[a].repo == w.nodes@[b].repo
        ==> footprint::disjoint(w, w.nodes@[a].footprint, w.nodes@[b].footprint)
}

/// #806: every real World::check rule, including gate, pool and ticket checks.
#[verifier::opaque]
pub(super) open spec fn accepted(w: World) -> bool {
    w.gates@.len() > 0 && w.pool_capacity > 0
        && (forall|r: int| 0 <= r < w.gates@.len() ==> w.gates@[r].w > 0
            && holders(w, RepoId(r as u64), w.nodes@.len()) <= w.gates@[r].w)
        && (forall|i: int| 0 <= i < w.nodes@.len() ==> retirement::node_rules(w, w.nodes@[i]))
        && (forall|a: int, b: int| 0 <= a < b < w.nodes@.len() ==> pair(w, a, b))
        && pool_users(w.nodes@, w.nodes@.len()) <= w.pool_capacity
        && (forall|a: int, b: int| 0 <= a < w.nodes@.len() && 0 <= b < w.nodes@.len() && a != b
            ==> (w.nodes@[a].ticket.is_some() ==> w.nodes@[a].ticket != w.nodes@[b].ticket))
}

/// #806 gate_world: retain four nodes and one repo; weaken all node construction
/// constants to complete consistency, small safe counters and fresh command IDs.
pub(super) open spec fn gate_world(w: World) -> bool {
    domain(w) && w.nodes@.len() == 4 && w.gates@.len() == 1 && w.file_sets@.len() == 0
        && accepted(w) && retirement::bounded(w) && storage_ordered(w) && fresh_commands(w)
}

/// State-table Queued merge row: exactly the unobstructed original merge input.
pub(super) open spec fn merge_input(w: World) -> bool {
    gate_world(w) && w.nodes@[0].phase == Phase::Queued && !w.nodes@[0].paused
        && w.nodes@[0].effect.is_none() && !w.nodes@[0].waiting_incident
        && w.nodes@[0].question.is_none()
}

/// State-table Resume: exact existing node update, including replay stuttering.
#[verifier::opaque]
pub(super) open spec fn resumed(n: Node, id: u64) -> Node {
    if id > n.last_cmd { Node { paused: false, last_cmd: id, ..n } } else { n }
}

/// State-table Resume: keep the exact update opaque in unrelated command arms.
#[verifier::opaque]
pub(super) open spec fn resume_post(before: Node, after: Node, id: u64, command: Command) -> bool {
    command == Command::Resume ==> after == resumed(before, id)
}

/// State-table Resume: the real apply dispatch accepts exactly an existing node.
pub(super) open spec fn resume_selected(w: World, event: Event) -> bool {
    match event {
        Event::Command { node, command: Command::Resume, .. } => node.0 < w.nodes@.len(),
        _ => false,
    }
}

/// State-table Resume: exact apply frame, opaque in unrelated event branches.
#[verifier::opaque]
pub(super) open spec fn resume_apply_post(before: World, after: World, event: Event) -> bool {
    match event {
        Event::Command { node, id, command: Command::Resume } => node.0 < before.nodes@.len() ==> {
            after.nodes@ == before.nodes@.update(node.0 as int, resumed(before.nodes@[node.0 as int], id))
                && after.next_question == before.next_question
                && after.file_sets@ == before.file_sets@ && after.dependencies@ == before.dependencies@
        },
        _ => true,
    }
}

/// State-table Resume row: all four original first-node phase alternatives.
pub(super) open spec fn resume_input(w: World) -> bool {
    let n = w.nodes@[0];
    gate_world(w) && n.paused && matches!(n.phase,
        Phase::Ready(Resume::Making) | Phase::Queued | Phase::Making | Phase::Unplanned)
}

/// #771 / #806: original small construction implies headroom and indexed edges.
/// No executable measure bridge is used here.
pub(super) proof fn gate_reserves(w: World)
    requires gate_world(w),
    ensures world_headroom(w), dependencies::edges_indexable(w),
{
    reveal(retirement::bounded); reveal(world_headroom);
    reveal_with_fuel(open_prefix, 5);
    open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
    assert forall|j: int| 0 <= j < w.nodes@.len() implies node_headroom(w.nodes@[j]) by {}
    dependencies::indexable_definition(w);
}

}
