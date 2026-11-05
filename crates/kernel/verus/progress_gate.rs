//! Q30/Q39/Q48 / #806: shared four-leaf gate fixture and complete checker bridge.
use super::*;

verus! {
/// #806: original gate storage. Kept separate from phase shape for C2 and Q48.
pub(super) open spec fn domain(w: World) -> bool {
    w.nodes@.len() == 4 && w.gates@.len() == 1 && w.dependencies@.len() == 0
        && w.file_sets@.len() == 0
        && forall|i: int| 0 <= i < 4 ==> !w.nodes@[i].credit.withheld
}

/// Q30: holders counted by the production capacity scan.
pub(super) open spec fn holders(w: World, end: nat) -> nat {
    potential_prefix(w.nodes@, end, |n: Node| if n.holds && n.repo == RepoId(0) { 1nat } else { 0nat })
}

/// Q39: pool occupancy counted by the production scan.
pub(super) open spec fn runs(w: World, end: nat) -> nat {
    potential_prefix(w.nodes@, end, |n: Node| if n.pool { 1nat } else { 0nat })
}

/// Q30/Q39: every pair check, including ticket uniqueness and footprint locks.
pub(super) open spec fn pair(w: World, a: int, b: int) -> bool {
    (w.nodes@[a].ticket.is_some() ==> w.nodes@[a].ticket != w.nodes@[b].ticket)
        && (w.nodes@[a].holds && w.nodes@[b].holds && w.nodes@[a].repo == w.nodes@[b].repo
            ==> footprint::disjoint(w, w.nodes@[a].footprint, w.nodes@[b].footprint))
}

/// #806: full real checker acceptance in domain, rather than world_checked's projection.
#[verifier::opaque]
pub(super) open spec fn accepted(w: World) -> bool {
    w.pool_capacity > 0 && w.gates@[0].w > 0 && holders(w, 4) <= w.gates@[0].w
        && runs(w, 4) <= w.pool_capacity
        && (forall|i: int| 0 <= i < 4 ==> retirement::node_rules(w, w.nodes@[i]))
        && (forall|a: int, b: int| 0 <= a < b < 4 ==> pair(w, a, b))
}

/// #806: exactly gate_world's node constructor, with symbolic phase, pause,
/// inline footprint and Ready ticket. All other Node::new fields stay fixed.
pub(super) open spec fn leaf(n: Node) -> bool {
    initial_node(Node { phase: Phase::Ingesting, pos: 0, footprint: footprint::inline_footprint(0),
        credit: Credit { ready: false, start: 0, ..n.credit }, attempt: 0, paused: false,
        holds: false, pool: false, worktree: false, run: None, head: None, checked: None,
        approved: None, had_pr: false, pr_open: false, ticket: None, ..n }, RepoId(0))
        && n.credit.ready && n.credit.start == 4 && n.attempt == 1
        && (exists|files: u64| 0 < files < 16 && n.footprint == footprint::inline_footprint(files))
        && match n.phase {
            Phase::Ready(Resume::Making) => n.pos == 0 && !n.holds && !n.pool && !n.worktree
                && n.run.is_none() && n.head.is_none() && n.checked.is_none() && n.approved.is_none()
                && !n.had_pr && !n.pr_open,
            Phase::Unplanned => n.pos == 0 && !n.holds && !n.pool && !n.worktree
                && n.run.is_none() && n.head.is_none() && n.checked.is_none() && n.approved.is_none()
                && !n.had_pr && !n.pr_open && n.ticket.is_none(),
            Phase::Making => n.pos == 2 && n.holds && n.pool && n.worktree
                && n.run == Some(n.generation) && n.head.is_none() && n.checked.is_none()
                && n.approved.is_none() && !n.had_pr && !n.pr_open && n.ticket.is_none(),
            Phase::Queued => n.pos == 0 && n.holds && !n.pool && n.worktree && n.run.is_none()
                && n.head == Some(HeadId(1)) && n.checked == n.head && n.approved == n.head
                && n.had_pr && n.pr_open && n.ticket.is_none(),
            _ => false,
        }
}

/// #806: gate_world with arbitrary positive width, preserving the entire original domain.
pub(super) open spec fn input(w: World) -> bool {
    domain(w) && accepted(w) && w.pool_capacity == 4 && w.next_ticket == 8
        && w.next_question == 0 && !w.fail_fast
        && forall|i: int| 0 <= i < 4 ==> leaf(w.nodes@[i])
}
}

kernel_fn! {
/// #806: the full production checker accepts the gate predicate.
pub(super) fn verify_checked(w: &World) -> (result: ()) [
    requires domain(*w), accepted(*w), world_headroom(*w), storage_ordered(*w),
    ensures world_checked(*w),
] {
    let checked = w.check();
    assert!(checked.is_ok());
}
}

verus! {
/// Maker completion row / #806: exact result of end_operation then MakerDone.
pub(super) open spec fn maker_completed(n: Node, head: HeadId) -> Node {
    retirement::phased(Node { run: None, job: None, pool: false, run_failures: 0,
        run_incidents: 0, head: Some(head), checked: None, ..n }, Phase::Checking)
}

/// #806: original constructor fields establish every enforced arithmetic reserve.
pub(super) proof fn input_scope(w: World)
    requires input(w),
    ensures world_headroom(w), storage_ordered(w), fresh_commands(w),
{
    reveal(initial_node); reveal(world_headroom); reveal(storage_ordered);
    reveal(footprint::ordered_table); reveal(fresh_commands);
    reveal_with_fuel(open_prefix, 5);
    assert forall|i: int| 0 <= i < 4 implies node_headroom(w.nodes@[i]) by {}
}
}

verus! {
/// #806: original four-leaf ranks fit u64; this is only a bound on #805's spec,
/// leaving the executable World::measure bridge to builder A.
pub(super) proof fn input_measure_bound(w: World)
    requires input(w),
    ensures progress_measure(w) <= 8000,
{
    reveal(initial_node); reveal(distance); reveal_with_fuel(phase_rank, 3);
    assert forall|i: int| 0 <= i < 4 implies distance(w.nodes@[i]) <= 2000 by {}
    reveal_with_fuel(node_total, 5);
    progress::empty_edges(w);
}
}
