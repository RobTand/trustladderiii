//! K4a: unbounded dependency safety over the production handlers.
//! Graph composition, complete-checker preservation by both dependency
//! handlers, and the arbitrary-world replacements of the five dependency
//! harnesses. Worlds have any number of nodes and declarations.
use super::*;

verus! {
// ------------------------------------------------------------------ graph

/// Reachability composes along an actual route.
pub(super) proof fn concat_prefix(w: World, a: NodeId, route: Seq<NodeId>, end: int)
    requires reachable(w, a, route[0]), path(w, route), 0 <= end < route.len(),
    ensures reachable(w, a, route[end]),
    decreases end,
{
    if end > 0 {
        concat_prefix(w, a, route, end - 1);
        reveal(path);
        assert(active_edge(w, route[end - 1], route[end]));
        lemmas::extend(w, a, route[end - 1], route[end]);
    }
}

/// Reachability is transitive.
pub(super) proof fn concat(w: World, a: NodeId, b: NodeId, c: NodeId)
    requires reachable(w, a, b), reachable(w, b, c),
    ensures reachable(w, a, c),
{
    reveal(reachable);
    let route = choose|route: Seq<NodeId>| path(w, route)
        && route[0] == b && route[route.len() - 1] == c;
    reveal(path);
    assert(route.len() > 0);
    concat_prefix(w, a, route, route.len() - 1);
}

/// Every edge of `after` is an edge of `before`, except possibly one new edge i->t.
pub(super) open spec fn edges_within(before: World, after: World, i: NodeId, t: NodeId,
    added: bool) -> bool {
    after.nodes@.len() == before.nodes@.len()
        && forall|a: NodeId, b: NodeId| #[trigger] active_edge(after, a, b)
            ==> active_edge(before, a, b) || (added && a == i && b == t)
}

/// A route in `after` is a route in `before`, or it crosses the new edge.
pub(super) proof fn reach_split(before: World, after: World, i: NodeId, t: NodeId, added: bool,
    route: Seq<NodeId>, end: int)
    requires edges_within(before, after, i, t, added), path(after, route), 0 <= end < route.len(),
    ensures reachable(before, route[0], route[end])
        || (added && reachable(before, route[0], i) && reachable(before, t, route[end])),
    decreases end,
{
    reveal(path);
    if end == 0 {
        lemmas::reflexive(before, route[0]);
    } else {
        reach_split(before, after, i, t, added, route, end - 1);
        let x = route[end - 1];
        let y = route[end];
        assert(active_edge(after, x, y));
        if active_edge(before, x, y) {
            if reachable(before, route[0], x) {
                lemmas::extend(before, route[0], x, y);
            } else {
                lemmas::extend(before, t, x, y);
            }
        } else {
            reveal(active_edge);
            assert(y.0 < before.nodes@.len());
            lemmas::reflexive(before, t);
        }
    }
}

/// K4a no-cycle: adding only an edge whose reverse is unreachable, and
/// removing any edges, keeps the production graph acyclic.
pub(super) proof fn acyclic_after(before: World, after: World, i: NodeId, t: NodeId, added: bool)
    requires acyclic(before), edges_within(before, after, i, t, added),
        added ==> !reachable(before, t, i),
    ensures acyclic(after),
{
    acyclic_definition(before);
    acyclic_definition(after);
    assert forall|a: NodeId, b: NodeId| active_edge(after, a, b) implies !reachable(after, b, a) by {
        if reachable(after, b, a) {
            reveal(reachable);
            let route = choose|route: Seq<NodeId>| path(after, route)
                && route[0] == b && route[route.len() - 1] == a;
            reveal(path);
            assert(route.len() > 0);
            reach_split(before, after, i, t, added, route, route.len() - 1);
            if active_edge(before, a, b) {
                if !reachable(before, b, a) {
                    lemmas::extend(before, t, a, b);
                    concat(before, t, b, i);
                }
            }
        }
    }
}

// ------------------------------------------------- non-dependency checker frame

proof fn holders_frame(before: World, after: World, repo: RepoId, end: nat)
    requires end <= before.nodes@.len(), after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() ==> (#[trigger] after.nodes@[j]).holds
            == before.nodes@[j].holds && after.nodes@[j].repo == before.nodes@[j].repo,
    ensures progress_gate_apply::holders(after, repo, end) == progress_gate_apply::holders(before, repo, end),
    decreases end,
{
    if end > 0 {
        holders_frame(before, after, repo, (end - 1) as nat);
        assert(after.nodes@[end - 1].holds == before.nodes@[end - 1].holds);
    }
}

proof fn pool_frame(before: Seq<Node>, after: Seq<Node>, end: nat)
    requires end <= before.len(), after.len() == before.len(),
        forall|j: int| 0 <= j < before.len() ==> (#[trigger] after[j]).pool == before[j].pool,
    ensures pool_users(after, end) == pool_users(before, end),
    decreases end,
{
    if end > 0 {
        pool_frame(before, after, (end - 1) as nat);
        assert(after[end - 1].pool == before[end - 1].pool);
    }
}

/// Changing only wait metadata and a permitted credit hold keeps every node rule.
proof fn owner_rules(w: World, n: Node, m: Node)
    requires retirement::node_rules(w, n),
        m == (Node { waiting_prerequisite: m.waiting_prerequisite,
            credit: Credit { withheld: m.credit.withheld, ..n.credit }, ..n }),
        m.credit.withheld ==> n.credit.withheld || (n.credit.ready
            && matches!(n.phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned)),
    ensures retirement::node_rules(w, m),
{
    reveal(retirement::node_rules);
}

/// The one place the dependency frame is revealed for the checker: node i keeps its rules,
/// and the frame's field facts are exported without the Node struct-update equality.
proof fn frame_owner(before: World, after: World, i: int)
    requires dependency_frame(before, after, i), 0 <= i < before.nodes@.len(),
        retirement::node_rules(before, before.nodes@[i]),
        after.nodes@[i].credit.withheld ==> before.nodes@[i].credit.withheld
            || (before.nodes@[i].credit.ready && early(before.nodes@[i].phase)),
    ensures retirement::node_rules(after, after.nodes@[i]),
        after.nodes@.len() == before.nodes@.len(), after.gates@ == before.gates@,
        after.file_sets@ == before.file_sets@, after.pool_capacity == before.pool_capacity,
        after.next_ticket == before.next_ticket, after.next_question == before.next_question,
        forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> #[trigger] after.nodes@[j] == before.nodes@[j],
        after.nodes@[i].holds == before.nodes@[i].holds, after.nodes@[i].repo == before.nodes@[i].repo,
        after.nodes@[i].pool == before.nodes@[i].pool,
        after.nodes@[i].footprint == before.nodes@[i].footprint,
        after.nodes@[i].ticket == before.nodes@[i].ticket,
{
    reveal(dependency_frame);
    retirement::node_frame(before, after, before.nodes@[i]);
    owner_rules(after, before.nodes@[i], after.nodes@[i]);
}

/// Every node rule survives when only node i changes and node i already keeps its rules.
proof fn frame_node_rules(before: World, after: World, i: int)
    requires 0 <= i < before.nodes@.len(), after.nodes@.len() == before.nodes@.len(),
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket, after.next_question == before.next_question,
        forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> #[trigger] after.nodes@[j] == before.nodes@[j],
        retirement::node_rules(after, after.nodes@[i]),
        forall|j: int| 0 <= j < before.nodes@.len()
            ==> retirement::node_rules(before, #[trigger] before.nodes@[j]),
    ensures forall|j: int| 0 <= j < after.nodes@.len()
            ==> retirement::node_rules(after, #[trigger] after.nodes@[j]),
{
    assert forall|j: int| 0 <= j < after.nodes@.len()
        implies retirement::node_rules(after, #[trigger] after.nodes@[j]) by {
        if j != i {
            retirement::node_frame(before, after, before.nodes@[j]);
            assert(retirement::node_rules(before, before.nodes@[j]));
        }
    }
}

/// Per-repository holder counts stay within width when holds and repos are unchanged.
proof fn frame_holders(before: World, after: World)
    requires after.nodes@.len() == before.nodes@.len(), after.gates@ == before.gates@,
        forall|j: int| 0 <= j < before.nodes@.len() ==> (#[trigger] after.nodes@[j]).holds
            == before.nodes@[j].holds && after.nodes@[j].repo == before.nodes@[j].repo,
        forall|r: int| 0 <= r < before.gates@.len() ==> before.gates@[r].w > 0
            && progress_gate_apply::holders(before, RepoId(r as u64), before.nodes@.len())
                <= before.gates@[r].w,
    ensures forall|r: int| 0 <= r < after.gates@.len() ==> after.gates@[r].w > 0
            && progress_gate_apply::holders(after, RepoId(r as u64), after.nodes@.len())
                <= after.gates@[r].w,
{
    assert forall|r: int| 0 <= r < after.gates@.len() implies after.gates@[r].w > 0
        && progress_gate_apply::holders(after, RepoId(r as u64), after.nodes@.len())
            <= after.gates@[r].w by {
        holders_frame(before, after, RepoId(r as u64), before.nodes@.len());
    }
}

/// Held same-repo footprints stay disjoint when holds, repos, footprints and storage are unchanged.
proof fn frame_pairs(before: World, after: World)
    requires after.nodes@.len() == before.nodes@.len(), after.file_sets@ == before.file_sets@,
        forall|j: int| 0 <= j < before.nodes@.len() ==> (#[trigger] after.nodes@[j]).holds
            == before.nodes@[j].holds && after.nodes@[j].repo == before.nodes@[j].repo
            && after.nodes@[j].footprint == before.nodes@[j].footprint,
        forall|a: int, b: int| 0 <= a < b < before.nodes@.len()
            ==> progress_gate_apply::pair(before, a, b),
    ensures forall|a: int, b: int| 0 <= a < b < after.nodes@.len()
            ==> progress_gate_apply::pair(after, a, b),
{
    assert forall|a: int, b: int| 0 <= a < b < after.nodes@.len()
        implies progress_gate_apply::pair(after, a, b) by {
        footprint::storage_frame(before, after, before.nodes@[a].footprint, before.nodes@[b].footprint);
        assert(progress_gate_apply::pair(before, a, b));
    }
}

/// The dependency handlers keep every non-dependency checker rule.
pub(super) proof fn checker_frame(before: World, after: World, i: int)
    requires progress_gate_apply::accepted(before), dependency_frame(before, after, i),
        0 <= i < before.nodes@.len(),
        after.nodes@[i].credit.withheld ==> before.nodes@[i].credit.withheld
            || (before.nodes@[i].credit.ready && early(before.nodes@[i].phase)),
    ensures progress_gate_apply::accepted(after),
{
    reveal(progress_gate_apply::accepted);
    let len = before.nodes@.len();
    frame_owner(before, after, i);
    assert forall|j: int| 0 <= j < len implies (#[trigger] after.nodes@[j]).holds == before.nodes@[j].holds
        && after.nodes@[j].repo == before.nodes@[j].repo && after.nodes@[j].pool == before.nodes@[j].pool
        && after.nodes@[j].footprint == before.nodes@[j].footprint
        && after.nodes@[j].ticket == before.nodes@[j].ticket by {}
    frame_node_rules(before, after, i);
    frame_holders(before, after);
    frame_pairs(before, after);
    pool_frame(before.nodes@, after.nodes@, len);
}

/// An accepted Unplanned node holds no slot, grant, place in line or run.
pub(super) proof fn unplanned_free(w: World, n: Node)
    requires retirement::node_rules(w, n), n.phase == Phase::Unplanned,
    ensures !n.holds && !n.pool && n.ticket.is_none() && n.run.is_none(), n.pos == 0,
        n.question.is_none(),
{
    reveal(retirement::node_rules);
    assert(items(Phase::Unplanned).len() == 1);
    assert(items(Phase::Unplanned)[0] == Item::Admit);
}

/// Pre-planning phases carry no question, so their waits are trivial.
proof fn early_waits(w: World, n: Node)
    requires retirement::node_rules(w, n),
        matches!(n.phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned),
    ensures n.question.is_none(), waits(n.question),
        !n.credit.ready ==> zero_credit(n.credit),
        !n.credit.ready ==> n.credit.start == 0 && n.credit.earned == 0
            && n.credit.added == 0 && n.credit.spent == 0,
{
    reveal(retirement::node_rules);
    reveal(zero_credit);
}

// ------------------------------------------------------------ table frame

/// Exact table equality plus equal endpoint fields preserve rules_ok.
pub(super) proof fn table_frame(before: World, after: World)
    requires rules_ok(before), after.dependencies@ == before.dependencies@,
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len()
            ==> same_endpoint(#[trigger] before.nodes@[j], after.nodes@[j]),
    ensures rules_ok(after),
{
    rules_definition(before);
    rules_definition(after);
    assert forall|n: NodeId| prerequisites_delivered(after, n) == prerequisites_delivered(before, n) by {
        prerequisite_definition(after, n);
        prerequisite_definition(before, n);
    }
    assert forall|j: int| 0 <= j < after.dependencies@.len()
        implies row_ok(after, #[trigger] after.dependencies@[j]) by {
        let d = after.dependencies@[j];
        assert(row_ok(before, before.dependencies@[j]));
        if d.node.0 < before.nodes@.len() {
            assert(same_endpoint(before.nodes@[d.node.0 as int], after.nodes@[d.node.0 as int]));
        }
        if let Some(t) = d.target.request {
            if t.0 < before.nodes@.len() {
                assert(same_endpoint(before.nodes@[t.0 as int], after.nodes@[t.0 as int]));
            }
        }
    }
    assert forall|j: int| 0 <= j < after.nodes@.len() && #[trigger] after.nodes@[j].credit.withheld
        implies exists|k: int| 0 <= k < after.dependencies@.len()
            && after.dependencies@[k].node == NodeId(j as u64) by {
        assert(same_endpoint(before.nodes@[j], after.nodes@[j]));
        assert(before.nodes@[j].credit.withheld);
    }
    assert forall|a: NodeId, b: NodeId| #[trigger] active_edge(after, a, b)
        implies active_edge(before, a, b) by {
        reveal(active_edge);
        assert(same_endpoint(before.nodes@[a.0 as int], after.nodes@[a.0 as int]));
    }
    acyclic_after(before, after, NodeId(0), NodeId(0), false);
}

// ------------------------------------------------------- declaration writes

pub(super) open spec fn slot(existing: Option<usize>, len: nat) -> int {
    match existing { Some(k) => k as int, None => len as int }
}

/// The write branch of fact_result, with the row the handler stored.
pub(super) open spec fn written(before: World, after: World, i: int, target: DependencyTarget,
    status: DependencyStatus, existing: Option<usize>) -> bool {
    target_ok(before, i, target) && found(before, i, target, existing)
        && writes_row(before, i, target, status, existing)
        && writes::write(before.dependencies@, after.dependencies@, existing,
            declared(before, i, target, status))
        && after.nodes@[i].credit.withheld == withheld_after(before.nodes@[i].credit.withheld, before.nodes@[i].planning_started,
            before.nodes@[i].phase, before.nodes@[i].credit.ready, after.dependencies@, NodeId(i as u64))
}

/// Exact row-level table facts of one handler write.
proof fn written_rows(before: World, after: World, i: int, target: DependencyTarget,
    status: DependencyStatus, existing: Option<usize>)
    requires written(before, after, i, target, status, existing),
    ensures after.dependencies@.len() == if existing is Some { before.dependencies@.len() }
            else { before.dependencies@.len() + 1 },
        0 <= slot(existing, before.dependencies@.len()) < after.dependencies@.len(),
        after.dependencies@[slot(existing, before.dependencies@.len())] == declared(before, i, target, status),
        forall|j: int| 0 <= j < before.dependencies@.len() && j != slot(existing, before.dependencies@.len())
            ==> (#[trigger] after.dependencies@[j]) == before.dependencies@[j],
{
    let d = declared(before, i, target, status);
    writes::length(before.dependencies@, after.dependencies@, existing, d);
    writes::index(before.dependencies@, after.dependencies@, existing, d);
    writes::frame(before.dependencies@, after.dependencies@, existing, d);
}

/// Every new graph edge is the declared i->target edge, admitted only when
/// its reverse is unreachable; Closed owners and node count are unchanged.
pub(super) proof fn written_edges(before: World, after: World, i: int, target: DependencyTarget,
    status: DependencyStatus, existing: Option<usize>)
    requires written(before, after, i, target, status, existing), dependency_frame(before, after, i),
        0 <= i < before.nodes@.len(), i <= u64::MAX, acyclic(before),
    ensures acyclic(after),
{
    reveal(dependency_frame);
    written_rows(before, after, i, target, status, existing);
    let d = declared(before, i, target, status);
    let s = slot(existing, before.dependencies@.len());
    let t = match target.request { Some(t) => t, None => NodeId(0) };
    let added = target.request.is_some() && status == DependencyStatus::Pending && !d.rejected;
    assert forall|a: NodeId, b: NodeId| #[trigger] active_edge(after, a, b)
        implies active_edge(before, a, b) || (added && a == NodeId(i as u64) && b == t) by {
        reveal(active_edge);
        let j = choose|j: int| 0 <= j < after.dependencies@.len()
            && after.dependencies@[j].node == a
            && after.dependencies@[j].target.request == Some(b)
            && !after.dependencies@[j].rejected
            && after.dependencies@[j].status == DependencyStatus::Pending;
        if j != s {
            assert(after.dependencies@[j] == before.dependencies@[j]);
        }
        assert(before.nodes@[a.0 as int].phase == after.nodes@[a.0 as int].phase);
    }
    acyclic_after(before, after, NodeId(i as u64), t, added);
}

/// The written table keeps every declaration key unique.
proof fn written_unique(before: World, after: World, i: int, target: DependencyTarget,
    status: DependencyStatus, existing: Option<usize>)
    requires written(before, after, i, target, status, existing), declarations_unique(before),
    ensures declarations_unique(after),
{
    written_rows(before, after, i, target, status, existing);
    let d = declared(before, i, target, status);
    let s = slot(existing, before.dependencies@.len());
    assert forall|a: int, b: int| 0 <= a < b < after.dependencies@.len()
        implies !same_declaration(after.dependencies@[a], after.dependencies@[b]) by {
        if a != s && b != s {
            assert(after.dependencies@[a] == before.dependencies@[a]);
            assert(after.dependencies@[b] == before.dependencies@[b]);
        } else if let Some(k) = existing {
            let other = if a == s { b } else { a };
            assert(after.dependencies@[other] == before.dependencies@[other]);
            if other < k as int {
                assert(!same_declaration(before.dependencies@[other], before.dependencies@[k as int]));
            } else {
                assert(!same_declaration(before.dependencies@[k as int], before.dependencies@[other]));
            }
        } else {
            assert(after.dependencies@[a] == before.dependencies@[a]);
            assert(!same_key(before.dependencies@[a], NodeId(i as u64), target.kind, target.number));
        }
    }
}

/// Every owner with withheld credit still owns a declaration.
proof fn written_credit(before: World, after: World, i: int, target: DependencyTarget,
    status: DependencyStatus, existing: Option<usize>)
    requires written(before, after, i, target, status, existing), dependency_frame(before, after, i),
        credit_declared(before), 0 <= i < before.nodes@.len(), before.nodes@.len() <= u64::MAX,
    ensures credit_declared(after),
{
    reveal(dependency_frame);
    written_rows(before, after, i, target, status, existing);
    let s = slot(existing, before.dependencies@.len());
    assert forall|j: int| 0 <= j < after.nodes@.len() && #[trigger] after.nodes@[j].credit.withheld
        implies exists|k: int| 0 <= k < after.dependencies@.len()
            && after.dependencies@[k].node == NodeId(j as u64) by {
        if j == i {
            assert(after.dependencies@[s].node == NodeId(j as u64));
        } else {
            assert(before.nodes@[j].credit.withheld);
            let k0 = choose|k: int| 0 <= k < before.dependencies@.len()
                && before.dependencies@[k].node == NodeId(j as u64);
            if k0 == s {
                assert(before.dependencies@[k0].node == NodeId(i as u64));
                assert(false);
            } else {
                assert(after.dependencies@[k0] == before.dependencies@[k0]);
            }
        }
    }
}

/// Rows owned by any other node, and their delivery state, are unchanged.
proof fn written_other(before: World, after: World, i: int, target: DependencyTarget,
    status: DependencyStatus, existing: Option<usize>, o: NodeId)
    requires written(before, after, i, target, status, existing), o != NodeId(i as u64),
    ensures delivered(after.dependencies@, o) == delivered(before.dependencies@, o),
{
    written_rows(before, after, i, target, status, existing);
    let s = slot(existing, before.dependencies@.len());
    delivered_definition(after.dependencies@, o);
    delivered_definition(before.dependencies@, o);
    assert forall|j: int| 0 <= j < after.dependencies@.len() && after.dependencies@[j].node == o
        implies j < before.dependencies@.len() && after.dependencies@[j] == before.dependencies@[j] by {
        if j == s { assert(after.dependencies@[j].node == NodeId(i as u64)); }
    }
    assert forall|j: int| 0 <= j < before.dependencies@.len() && before.dependencies@[j].node == o
        implies after.dependencies@[j] == before.dependencies@[j] by {
        if j == s {
            assert(before.dependencies@[j].node == NodeId(i as u64));
        }
    }
}

/// A blocked stored row was admitted only at a pre-planning or teardown owner.
proof fn written_blocked(before: World, after: World, i: int, target: DependencyTarget,
    status: DependencyStatus, existing: Option<usize>)
    requires written(before, after, i, target, status, existing),
        forall|j: int| 0 <= j < before.dependencies@.len()
            ==> row_ok(before, #[trigger] before.dependencies@[j]),
        retirement::node_rules(before, before.nodes@[i]), 0 <= i < before.nodes@.len(),
        i <= u64::MAX,
    ensures (declared(before, i, target, status).rejected
            || declared(before, i, target, status).status != DependencyStatus::Delivered)
            ==> !before.nodes@[i].planning_started && waits(before.nodes@[i].question)
                && blocked_phase(before.nodes@[i].phase),
{
    let n = before.nodes@[i];
    match existing {
        None => { early_waits(before, n); }
        Some(k) => {
            let previous = before.dependencies@[k as int];
            assert(row_ok(before, previous));
            assert(previous.node == NodeId(i as u64));
            assert(previous.node.0 as int == i);
            if revocable_owner(n) {
                assert(!n.planning_started && waits(n.question) && blocked_phase(n.phase));
            }
        }
    }
}

/// Every stored row passes every per-declaration check.
proof fn written_row_rules(before: World, after: World, i: int, target: DependencyTarget,
    status: DependencyStatus, existing: Option<usize>)
    requires written(before, after, i, target, status, existing), dependency_frame(before, after, i),
        forall|j: int| 0 <= j < before.dependencies@.len()
            ==> row_ok(before, #[trigger] before.dependencies@[j]),
        retirement::node_rules(before, before.nodes@[i]),
        0 <= i < before.nodes@.len(), before.nodes@.len() <= u64::MAX,
    ensures forall|j: int| 0 <= j < after.dependencies@.len()
            ==> row_ok(after, #[trigger] after.dependencies@[j]),
{
    reveal(withheld_after);
    reveal(dependency_frame);
    written_rows(before, after, i, target, status, existing);
    written_blocked(before, after, i, target, status, existing);
    let d = declared(before, i, target, status);
    let s = slot(existing, before.dependencies@.len());
    let owner = NodeId(i as u64);
    let n = before.nodes@[i];
    let m = after.nodes@[i];
    prerequisite_definition(after, owner);
    prerequisite_definition(before, owner);
    reveal(zero_credit);
    // The owner's blocked-credit rule after its reconciled hold.
    assert((!delivered(after.dependencies@, owner)
        && matches!(n.phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned))
        ==> zero_credit(m.credit)) by {
    if !delivered(after.dependencies@, owner)
        && matches!(n.phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned) {
        early_waits(before, n);
        if !m.credit.withheld && n.credit.ready {
            assert(n.planning_started);
            assert(!delivered(before.dependencies@, owner)) by {
                delivered_definition(after.dependencies@, owner);
                delivered_definition(before.dependencies@, owner);
                if delivered(before.dependencies@, owner) {
                    let j = choose|j: int| 0 <= j < after.dependencies@.len()
                        && after.dependencies@[j].node == owner
                        && !(!after.dependencies@[j].rejected
                            && after.dependencies@[j].status == DependencyStatus::Delivered);
                    if j != s { assert(after.dependencies@[j] == before.dependencies@[j]); }
                }
            }
            delivered_definition(before.dependencies@, owner);
            let j0 = choose|j: int| 0 <= j < before.dependencies@.len()
                && before.dependencies@[j].node == owner
                && !(!before.dependencies@[j].rejected
                    && before.dependencies@[j].status == DependencyStatus::Delivered);
            assert(row_ok(before, before.dependencies@[j0]));
        }
    }
    }
    if n.phase == Phase::Unplanned { unplanned_free(before, n); }
    assert forall|j: int| 0 <= j < after.dependencies@.len()
        implies row_ok(after, #[trigger] after.dependencies@[j]) by {
        let x = after.dependencies@[j];
        if j != s { assert(x == before.dependencies@[j]); assert(row_ok(before, x)); }
        if x.node != owner && x.node.0 < before.nodes@.len() {
            assert(x.node.0 != i);
            written_other(before, after, i, target, status, existing, x.node);
            prerequisite_definition(after, x.node);
            prerequisite_definition(before, x.node);
        }
    }
}

/// K4a registration/no-cycle: the real dependency_fact keeps rules_ok.
pub(super) proof fn fact_rules(before: World, after: World, i: int, target: DependencyTarget,
    status: DependencyStatus)
    requires rules_ok(before), progress_gate_apply::accepted(before),
        dependency_frame(before, after, i), fact_result(before, after, i, target, status),
        0 <= i < before.nodes@.len(), before.nodes@.len() <= u64::MAX,
    ensures rules_ok(after),
        after.nodes@[i].credit.withheld ==> before.nodes@[i].credit.withheld
            || (before.nodes@[i].credit.ready && early(before.nodes@[i].phase)),
{
    reveal(withheld_after);
    reveal(fact_result);
    reveal(progress_gate_apply::accepted);
    assert(retirement::node_rules(before, before.nodes@[i]));
    if after.dependencies@ == before.dependencies@
        && after.nodes@[i].credit.withheld == before.nodes@[i].credit.withheld {
        reveal(dependency_frame);
        assert forall|j: int| 0 <= j < before.nodes@.len()
            implies same_endpoint(#[trigger] before.nodes@[j], after.nodes@[j]) by {
            reveal(zero_credit);
        }
        table_frame(before, after);
    } else {
        let existing = choose|existing: Option<usize>| found(before, i, target, existing)
            && if writes_row(before, i, target, status, existing) {
                writes::write(before.dependencies@, after.dependencies@, existing,
                    declared(before, i, target, status))
                && after.nodes@[i].credit.withheld == withheld_after(before.nodes@[i].credit.withheld, before.nodes@[i].planning_started,
                    before.nodes@[i].phase, before.nodes@[i].credit.ready, after.dependencies@, NodeId(i as u64))
            } else {
                after.dependencies@ == before.dependencies@
                    && after.nodes@[i].credit.withheld == before.nodes@[i].credit.withheld
            };
        assert(written(before, after, i, target, status, existing));
        rules_definition(before);
        rules_definition(after);
        written_unique(before, after, i, target, status, existing);
        written_credit(before, after, i, target, status, existing);
        written_row_rules(before, after, i, target, status, existing);
        written_edges(before, after, i, target, status, existing);
    }
}

// ------------------------------------------------------------- removal

pub(super) open spec fn seq_unique(s: Seq<Dependency>) -> bool {
    forall|a: int, b: int| 0 <= a < b < s.len() ==> !same_declaration(s[a], s[b])
}

/// Compaction keeps only original rows that do not match the removed key.
pub(super) proof fn kept_rows(s: Seq<Dependency>, node: NodeId, kind: PrerequisiteKind, number: u64)
    ensures forall|a: int| 0 <= a < kept(s, node, kind, number).len() ==> exists|b: int| 0 <= b < s.len()
            && (#[trigger] kept(s, node, kind, number)[a]) == s[b] && !same_key(s[b], node, kind, number),
        forall|b: int| 0 <= b < s.len() && !same_key(#[trigger] s[b], node, kind, number)
            ==> exists|a: int| 0 <= a < kept(s, node, kind, number).len()
                && kept(s, node, kind, number)[a] == s[b],
        seq_unique(s) ==> seq_unique(kept(s, node, kind, number)),
    decreases s.len(),
{
    if s.len() > 0 {
        let p = s.drop_last();
        kept_rows(p, node, kind, number);
        let k = kept(s, node, kind, number);
        let kp = kept(p, node, kind, number);
        assert forall|b: int| 0 <= b < p.len() implies #[trigger] p[b] == s[b] by {}
        assert forall|a: int| 0 <= a < k.len() implies exists|b: int| 0 <= b < s.len()
            && (#[trigger] k[a]) == s[b] && !same_key(s[b], node, kind, number) by {
            if a < kp.len() {
                assert(k[a] == kp[a]);
                let b = choose|b: int| 0 <= b < p.len() && kp[a] == p[b] && !same_key(p[b], node, kind, number);
                assert(s[b] == p[b]);
            } else {
                assert(k[a] == s[s.len() - 1]);
            }
        }
        assert forall|b: int| 0 <= b < s.len() && !same_key(#[trigger] s[b], node, kind, number)
            implies exists|a: int| 0 <= a < k.len() && k[a] == s[b] by {
            if b < p.len() {
                assert(p[b] == s[b]);
                let a = choose|a: int| 0 <= a < kp.len() && kp[a] == p[b];
                assert(k[a] == kp[a]);
            } else {
                assert(k[kp.len() as int] == s[b]);
            }
        }
        if seq_unique(s) {
            assert(seq_unique(p));
            assert forall|a: int, c: int| 0 <= a < c < k.len()
                implies !same_declaration(k[a], k[c]) by {
                if c < kp.len() {
                    assert(k[a] == kp[a] && k[c] == kp[c]);
                } else {
                    assert(k[a] == kp[a]);
                    let b = choose|b: int| 0 <= b < p.len() && kp[a] == p[b] && !same_key(p[b], node, kind, number);
                    assert(!same_declaration(s[b], s[s.len() - 1]));
                }
            }
        }
    }
}

/// K4a removal: the real remove_dependency keeps rules_ok.
pub(super) proof fn remove_rules(before: World, after: World, i: int, kind: PrerequisiteKind, number: u64)
    requires rules_ok(before), progress_gate_apply::accepted(before),
        dependency_frame(before, after, i), remove_result(before, after, i, kind, number),
        0 <= i < before.nodes@.len(), before.nodes@.len() <= u64::MAX,
    ensures rules_ok(after),
        after.nodes@[i].credit.withheld ==> before.nodes@[i].credit.withheld
            || (before.nodes@[i].credit.ready && early(before.nodes@[i].phase)),
{
    reveal(remove_result);
    reveal(dependency_frame);
    reveal(progress_gate_apply::accepted);
    let n = before.nodes@[i];
    let m = after.nodes@[i];
    let owner = NodeId(i as u64);
    assert(retirement::node_rules(before, n));
    if n.phase != Phase::Ingesting || n.planning_started {
        assert forall|j: int| 0 <= j < before.nodes@.len()
            implies same_endpoint(#[trigger] before.nodes@[j], after.nodes@[j]) by {
            reveal(zero_credit);
        }
        table_frame(before, after);
    } else {
        let s = before.dependencies@;
        let t = after.dependencies@;
        kept_rows(s, owner, kind, number);
        rules_definition(before);
        rules_definition(after);
        early_waits(before, n);
        // Unique keys survive a stable subsequence.
        assert(seq_unique(s));
        assert(seq_unique(t));
        // Rows of other owners, and their delivery, are unchanged.
        assert forall|o: NodeId| o != owner implies delivered(t, o) == delivered(s, o) by {
            delivered_definition(t, o);
            delivered_definition(s, o);
            if delivered(s, o) {
                assert forall|a: int| 0 <= a < t.len() && t[a].node == o
                    implies !t[a].rejected && t[a].status == DependencyStatus::Delivered by {
                    let b = choose|b: int| 0 <= b < s.len() && t[a] == s[b]
                        && !same_key(s[b], owner, kind, number);
                }
            }
            if delivered(t, o) {
                assert forall|b: int| 0 <= b < s.len() && s[b].node == o
                    implies !s[b].rejected && s[b].status == DependencyStatus::Delivered by {
                    assert(!same_key(s[b], owner, kind, number));
                    let a = choose|a: int| 0 <= a < t.len() && t[a] == s[b];
                }
            }
        }
        prerequisite_definition(after, owner);
        reveal(zero_credit);
        assert(m.credit.start == n.credit.start && m.credit.earned == n.credit.earned
            && m.credit.added == n.credit.added && m.credit.spent == n.credit.spent);
        assert((!delivered(t, owner) && early(n.phase)) ==> zero_credit(m.credit));
        assert forall|a: int| 0 <= a < t.len() implies row_ok(after, #[trigger] t[a]) by {
            let b = choose|b: int| 0 <= b < s.len() && t[a] == s[b] && !same_key(s[b], owner, kind, number);
            let x = s[b];
            assert(row_ok(before, x));
            prerequisite_definition(after, x.node);
            prerequisite_definition(before, x.node);
            if x.node.0 < before.nodes@.len() {
                if x.node == owner {
                    assert(x.node.0 as int == i);
                } else {
                    assert(x.node.0 as int != i);
                    assert(delivered(t, x.node) == delivered(s, x.node));
                    assert(after.nodes@[x.node.0 as int] == before.nodes@[x.node.0 as int]);
                }
            }
            if let Some(r) = x.target.request {
                if r.0 < before.nodes@.len() {
                    assert(after.nodes@[r.0 as int].repo == before.nodes@[r.0 as int].repo);
                }
            }
        }
        assert forall|j: int| 0 <= j < after.nodes@.len() && #[trigger] after.nodes@[j].credit.withheld
            implies exists|k: int| 0 <= k < t.len() && t[k].node == NodeId(j as u64) by {
            if j == i {
                delivered_definition(t, owner);
            } else {
                assert(before.nodes@[j].credit.withheld);
                let b = choose|b: int| 0 <= b < s.len() && s[b].node == NodeId(j as u64);
                assert(!same_key(s[b], owner, kind, number));
                let a = choose|a: int| 0 <= a < t.len() && t[a] == s[b];
            }
        }
        assert forall|a: NodeId, b: NodeId| #[trigger] active_edge(after, a, b)
            implies active_edge(before, a, b) by {
            reveal(active_edge);
            let j = choose|j: int| 0 <= j < t.len() && t[j].node == a && t[j].target.request == Some(b)
                && !t[j].rejected && t[j].status == DependencyStatus::Pending;
            let k = choose|k: int| 0 <= k < s.len() && t[j] == s[k] && !same_key(s[k], owner, kind, number);
            assert(before.nodes@[a.0 as int].phase == after.nodes@[a.0 as int].phase);
        }
        acyclic_after(before, after, NodeId(0), NodeId(0), false);
    }
}

// ------------------------------------------------- exact handler table results

/// With unique keys, the real lookup determines the handler's exact table.
pub(super) proof fn fact_table(before: World, after: World, i: int, target: DependencyTarget,
    status: DependencyStatus, existing: Option<usize>)
    requires fact_result(before, after, i, target, status),
        declarations_unique(before) || existing is None,
        found(before, i, target, existing),
    ensures after.dependencies@ == if target_ok(before, i, target)
            && writes_row(before, i, target, status, existing) {
            match existing {
                Some(k) => before.dependencies@.update(k as int, declared(before, i, target, status)),
                None => before.dependencies@.push(declared(before, i, target, status)),
            }
        } else { before.dependencies@ },
{
    reveal(fact_result);
    if target_ok(before, i, target) {
        let e = choose|e: Option<usize>| found(before, i, target, e)
            && if writes_row(before, i, target, status, e) {
                writes::write(before.dependencies@, after.dependencies@, e,
                    declared(before, i, target, status))
                && after.nodes@[i].credit.withheld == withheld_after(before.nodes@[i].credit.withheld, before.nodes@[i].planning_started,
                    before.nodes@[i].phase, before.nodes@[i].credit.ready, after.dependencies@, NodeId(i as u64))
            } else {
                after.dependencies@ == before.dependencies@
                    && after.nodes@[i].credit.withheld == before.nodes@[i].credit.withheld
            };
        same_lookup(before, i, target, existing, e);
        if writes_row(before, i, target, status, existing) {
            writes::finish(before.dependencies@, after.dependencies@, existing,
                declared(before, i, target, status));
        }
    }
}

/// Two lookups of one key agree when keys are unique, and a fresh key has no row.
proof fn same_lookup(w: World, i: int, target: DependencyTarget, a: Option<usize>, b: Option<usize>)
    requires found(w, i, target, a), found(w, i, target, b),
        declarations_unique(w) || a is None || b is None,
    ensures a == b,
{
    match (a, b) {
        (Some(x), Some(y)) => {
            if x < y {
                assert(!same_declaration(w.dependencies@[x as int], w.dependencies@[y as int]));
            } else if y < x {
                assert(!same_declaration(w.dependencies@[y as int], w.dependencies@[x as int]));
            }
        },
        (None, Some(y)) => { assert(same_key(w.dependencies@[y as int], NodeId(i as u64), target.kind, target.number)); },
        (Some(x), None) => { assert(same_key(w.dependencies@[x as int], NodeId(i as u64), target.kind, target.number)); },
        (None, None) => {},
    }
}

/// K4a no-cycle without any other checker premise: the real handler keeps the
/// production graph acyclic and indexable; a fresh row is appended exactly.
pub(super) proof fn fact_graph(before: World, after: World, i: int, target: DependencyTarget,
    status: DependencyStatus)
    requires acyclic(before), edges_indexable(before), dependency_frame(before, after, i),
        fact_result(before, after, i, target, status), 0 <= i < before.nodes@.len(), i <= u64::MAX,
    ensures acyclic(after), edges_indexable(after),
        found(before, i, target, None) && target_ok(before, i, target)
            && writes_row(before, i, target, status, None)
            ==> after.dependencies@ == before.dependencies@.push(declared(before, i, target, status)),
{
    reveal(dependency_frame);
    if found(before, i, target, None) {
        fact_table(before, after, i, target, status, None);
    }
    reveal(fact_result);
    if after.dependencies@ == before.dependencies@ {
        indexability_frame(before, after);
        assert forall|a: NodeId, b: NodeId| #[trigger] active_edge(after, a, b)
            implies active_edge(before, a, b) by {
            reveal(active_edge);
            assert(before.nodes@[a.0 as int].phase == after.nodes@[a.0 as int].phase);
        }
        acyclic_after(before, after, NodeId(0), NodeId(0), false);
    } else {
        let e = choose|e: Option<usize>| found(before, i, target, e)
            && if writes_row(before, i, target, status, e) {
                writes::write(before.dependencies@, after.dependencies@, e,
                    declared(before, i, target, status))
                && after.nodes@[i].credit.withheld == withheld_after(before.nodes@[i].credit.withheld, before.nodes@[i].planning_started,
                    before.nodes@[i].phase, before.nodes@[i].credit.ready, after.dependencies@, NodeId(i as u64))
            } else {
                after.dependencies@ == before.dependencies@
                    && after.nodes@[i].credit.withheld == before.nodes@[i].credit.withheld
            };
        assert(written(before, after, i, target, status, e));
        written_edges(before, after, i, target, status, e);
        written_rows(before, after, i, target, status, e);
        let sl = slot(e, before.dependencies@.len());
        reveal(edges_indexable);
        assert forall|j: int| 0 <= j < after.dependencies@.len() implies
            after.dependencies@[j].node.0 < after.nodes@.len()
            && match after.dependencies@[j].target.request {
                None => true, Some(node) => node.0 < after.nodes@.len(),
            } by {
            if j != sl { assert(after.dependencies@[j] == before.dependencies@[j]); }
        }
    }
}

/// Removal never adds a graph edge and keeps every row indexable.
pub(super) proof fn remove_table(before: World, after: World, i: int, kind: PrerequisiteKind, number: u64)
    requires remove_result(before, after, i, kind, number),
    ensures after.dependencies@ == if before.nodes@[i].phase == Phase::Ingesting
            && !before.nodes@[i].planning_started {
            kept(before.dependencies@, NodeId(i as u64), kind, number)
        } else { before.dependencies@ },
{
    reveal(remove_result);
}

// ------------------------------------------------------ world frames

/// The dependency handlers keep the step-boundary headroom and file storage.
pub(super) proof fn reserve_frame(before: World, after: World, i: int)
    requires dependency_frame(before, after, i), 0 <= i < before.nodes@.len(),
        world_headroom(before), storage_ordered(before),
    ensures world_headroom(after), storage_ordered(after),
{
    reveal(dependency_frame);
    reveal(world_headroom);
    reveal(storage_ordered);
    reveal(footprint::ordered_table);
    open_prefix_compare(before.nodes@, after.nodes@, before.nodes@.len());
    assert forall|j: int| 0 <= j < after.nodes@.len() implies node_headroom(#[trigger] after.nodes@[j]) by {
        assert(node_headroom(before.nodes@[j]));
    }
}

/// Step-boundary reserves give the real scheduler's preconditions.
proof fn scheduler_ready(w: World, i: int)
    requires checking::accepted(w), world_headroom(w), storage_ordered(w), 0 <= i < w.nodes@.len(),
    ensures scheduling_safe(w),
        advance_pre(w.nodes@[i], w.next_ticket, 0),
{
    checking::accepted_checked(w);
    reveal(storage_ordered);
    checked_reserves(w);
    reveal(settle_ready);
    potential_member(w.nodes@, i, w.nodes@.len(), |n: Node| ticket_capacity(n));
}

// ------------------------------------------- the five Kani harness replacements

/// Replaces check_dependency_registration: any accepted world, node,
/// declaration and status, through the real handler. A started or
/// post-admission owner never gains a new declaration; an unstarted owner
/// gains exactly the declared row; complete acceptance is preserved.
pub(super) fn registration(mut world: World, i: usize, target: DependencyTarget,
    status: DependencyStatus) -> (result: (World, Outbox))
    requires checking::accepted(world), i < world.nodes@.len(), world.nodes@.len() <= u64::MAX,
    ensures checking::accepted(result.0), result.1.len <= 1,
        dependency_frame(world, result.0, i as int),
        forall|e: Option<usize>| found(world, i as int, target, e) ==> result.0.dependencies@
            == if target_ok(world, i as int, target) && writes_row(world, i as int, target, status, e) {
                match e {
                    Some(k) => world.dependencies@.update(k as int, declared(world, i as int, target, status)),
                    None => world.dependencies@.push(declared(world, i as int, target, status)),
                }
            } else { world.dependencies@ },
{
    let ghost before = world;
    let mut out = Outbox::new();
    proof {
        reveal(empty_outbox);
        checking::accepted_parts(before);
        rules_checked(before);
        rules_definition(before);
    }
    world.dependency_fact(i, target, status, &mut out);
    proof {
        fact_preserves(before, world, i as int, target, status);
        assert forall|e: Option<usize>| found(before, i as int, target, e) implies world.dependencies@
            == if target_ok(before, i as int, target) && writes_row(before, i as int, target, status, e) {
                match e {
                    Some(k) => before.dependencies@.update(k as int, declared(before, i as int, target, status)),
                    None => before.dependencies@.push(declared(before, i as int, target, status)),
                }
            } else { before.dependencies@ } by {
            fact_table(before, world, i as int, target, status, e);
        }
    }
    (world, out)
}

/// Replaces check_dependency_cycles: any acyclic indexable world, any owner
/// and target. The real graph query accepts the result; a fresh Pending
/// request is rejected exactly when its reverse path already exists.
pub(super) fn no_cycle(mut world: World, i: usize, target: DependencyTarget,
    status: DependencyStatus) -> (result: (World, bool))
    requires edges_indexable(world), acyclic(world), i < world.nodes@.len(), i <= u64::MAX,
    ensures result.1, acyclic(result.0),
        found(world, i as int, target, None) && target_ok(world, i as int, target)
            && writes_row(world, i as int, target, status, None)
            ==> result.0.dependencies@ == world.dependencies@.push(declared(world, i as int, target, status)),
{
    let ghost before = world;
    let mut out = Outbox::new();
    proof { reveal(empty_outbox); }
    world.dependency_fact(i, target, status, &mut out);
    proof { fact_graph(before, world, i as int, target, status); }
    let accepted = world.dependency_graph_acyclic();
    (world, accepted)
}

/// Replaces check_dependency_remove: any accepted world. Only an unstarted
/// Ingesting owner removes its matching declaration; every other row keeps
/// its identity, terminal outcome and order; complete acceptance is preserved.
pub(super) fn removal(mut world: World, i: usize, kind: PrerequisiteKind, number: u64)
    -> (result: World)
    requires checking::accepted(world), i < world.nodes@.len(), world.nodes@.len() <= u64::MAX,
    ensures checking::accepted(result),
        result.dependencies@ == if world.nodes@[i as int].phase == Phase::Ingesting
            && !world.nodes@[i as int].planning_started {
            kept(world.dependencies@, NodeId(i as u64), kind, number)
        } else { world.dependencies@ },
        forall|b: int| 0 <= b < world.dependencies@.len()
            && !same_key(#[trigger] world.dependencies@[b], NodeId(i as u64), kind, number)
            ==> exists|a: int| 0 <= a < result.dependencies@.len()
                && result.dependencies@[a] == world.dependencies@[b],
        forall|a: int| 0 <= a < result.dependencies@.len() ==> exists|b: int|
            0 <= b < world.dependencies@.len() && (#[trigger] result.dependencies@[a]) == world.dependencies@[b],
{
    let ghost before = world;
    world.remove_dependency(i, kind, number);
    proof {
        field_frame(before, world, i as int);
        remove_preserves(before, world, i as int, kind, number);
        remove_table(before, world, i as int, kind, number);
        kept_rows(before.dependencies@, NodeId(i as u64), kind, number);
        if world.dependencies@ == before.dependencies@ {
            assert forall|b: int| 0 <= b < before.dependencies@.len()
                && !same_key(#[trigger] before.dependencies@[b], NodeId(i as u64), kind, number)
                implies exists|a: int| 0 <= a < world.dependencies@.len()
                    && world.dependencies@[a] == before.dependencies@[b] by {
                assert(world.dependencies@[b] == before.dependencies@[b]);
            }
            assert forall|a: int| 0 <= a < world.dependencies@.len() implies exists|b: int|
                0 <= b < before.dependencies@.len()
                    && (#[trigger] world.dependencies@[a]) == before.dependencies@[b] by {
                assert(world.dependencies@[a] == before.dependencies@[a]);
            }
        }
    }
    world
}

/// Replaces check_dependency_waiting: in any accepted world with step
/// reserves, an Unplanned owner with an undelivered prerequisite holds no
/// slot, grant, place in line or run and has zero balance; the real
/// scheduler refuses to advance it, emits nothing and changes nothing.
pub(super) fn waiting_holds_nothing(mut world: World, i: usize)
    -> (result: (World, bool, Outbox, i128))
    requires checking::accepted(world), world_headroom(world), storage_ordered(world),
        i < world.nodes@.len(), i <= u64::MAX,
        world.nodes@[i as int].phase == Phase::Unplanned,
        !prerequisites_delivered(world, NodeId(i as u64)),
    ensures !result.1, result.2.len == 0, result.3 == 0,
        !world.nodes@[i as int].holds && !world.nodes@[i as int].pool
            && world.nodes@[i as int].ticket.is_none() && world.nodes@[i as int].run.is_none(),
        result.0.nodes@ == world.nodes@, result.0.dependencies@ == world.dependencies@,
        result.0.next_ticket == world.next_ticket, result.0.gates@ == world.gates@,
        result.0.file_sets@ == world.file_sets@,
        checking::accepted(result.0),
{
    let ghost before = world;
    proof {
        checking::accepted_parts(before);
        checking::accepted_node(before, i as int);
        unplanned_free(before, before.nodes@[i as int]);
        rules_definition(before);
        prerequisite_definition(before, NodeId(i as u64));
        delivered_definition(before.dependencies@, NodeId(i as u64));
        let j = choose|j: int| 0 <= j < before.dependencies@.len()
            && before.dependencies@[j].node == NodeId(i as u64)
            && !(!before.dependencies@[j].rejected
                && before.dependencies@[j].status == DependencyStatus::Delivered);
        assert(row_ok(before, before.dependencies@[j]));
        reveal(zero_credit);
        scheduler_ready(before, i as int);
    }
    let balance = world.nodes[i].credit.balance();
    let mut out = Outbox::new();
    proof { reveal(empty_outbox); }
    let moved = advance(&mut world, i, &mut out, &mut Report::new());
    proof {
        reveal(advance_post);
        assert(world.nodes@ =~= before.nodes@);
        checking::accepted_frame(before, world);
    }
    (world, moved, out, balance)
}

/// Replaces check_dependency_delivery, first half, through the real handler:
/// in any accepted world with step reserves, delivering a pending prerequisite
/// changes exactly that row, strictly lowers the progress measure and keeps
/// complete acceptance and the step reserves.
pub(super) fn delivered_releases(mut world: World, i: usize, target: DependencyTarget)
    -> (result: World)
    requires checking::accepted(world), world_headroom(world), storage_ordered(world),
        i < world.nodes@.len(), world.nodes@.len() <= u64::MAX,
        delivery_input(world, i as int, target),
    ensures progress::progress_measure(result) < progress::progress_measure(world),
        checking::accepted(result), world_headroom(result), storage_ordered(result),
        delivery_changed(world, result, i as int, target),
        dependency_frame(world, result, i as int),
{
    let ghost before = world;
    let mut out = Outbox::new();
    proof {
        reveal(empty_outbox);
        checking::accepted_parts(before);
        rules_checked(before);
    }
    world.dependency_fact(i, target, DependencyStatus::Delivered, &mut out);
    proof {
        fact_preserves(before, world, i as int, target, DependencyStatus::Delivered);
        reserve_frame(before, world, i as int);
    }
    world
}

/// Replaces check_dependency_delivery, second half, through the real
/// scheduler: an accepted Unplanned owner is admitted exactly when it is
/// unpaused, idle, every prerequisite is delivered and its gate has room.
/// Admission enters Planning, records planning start and releases withheld
/// credit with every ledger amount unchanged; refusal changes nothing.
pub(super) fn delivered_admits(mut world: World, i: usize) -> (result: (World, bool))
    requires checking::accepted(world), world_headroom(world), storage_ordered(world),
        i < world.nodes@.len(), i <= u64::MAX,
        world.nodes@[i as int].phase == Phase::Unplanned,
    ensures result.1 == (!world.nodes@[i as int].paused && !world.nodes@[i as int].waiting_incident
            && world.nodes@[i as int].effect.is_none()
            && delivered(world.dependencies@, NodeId(i as u64))
            && world.nodes@[i as int].repo.0 < world.gates@.len()
            && slot_waiters(world.nodes@, world.nodes@.len(), world.nodes@[i as int].repo)
                < world.gates@[world.nodes@[i as int].repo.0 as int].w),
        result.1 ==> result.0.nodes@[i as int].phase == (Phase::Planning { must_split: false })
            && result.0.nodes@[i as int].planning_started
            && result.0.nodes@[i as int].credit
                == (Credit { withheld: false, ..world.nodes@[i as int].credit }),
        !result.1 ==> result.0.nodes@ == world.nodes@ && result.0.next_ticket == world.next_ticket,
        result.0.dependencies@ == world.dependencies@,
        result.0.nodes@.len() == world.nodes@.len(),
{
    let ghost before = world;
    proof {
        checking::accepted_node(before, i as int);
        unplanned_free(before, before.nodes@[i as int]);
        scheduler_ready(before, i as int);
    }
    let mut out = Outbox::new();
    proof { reveal(empty_outbox); }
    let moved = advance(&mut world, i, &mut out, &mut Report::new());
    proof {
        reveal(advance_post);
        if !moved { assert(world.nodes@ =~= before.nodes@); }
    }
    (world, moved)
}

// ------------------------------------------- executable covers (nonvacuity)

/// The original one-root fixture is completely accepted by the real checker.
fn fixture_accepted(world: &World)
    requires dependency_retirement::domain(*world), dependency_retirement::accepted(*world),
    ensures checking::accepted(*world), storage_ordered(*world),
{
    proof {
        reveal(storage_ordered);
        reveal(footprint::ordered_table);
        reveal(dependency_retirement_domain);
        reveal(dependency_retirement_accepted);
    }
    let checked = world.check();
    assert(checked.is_ok());
}

/// The original fixture also has the step-boundary reserves.
proof fn fixture_reserves(world: World)
    requires dependency_retirement::domain(world), dependency_retirement::pristine(world.nodes@[0]),
        world.nodes@[0].generation == 0, world.nodes@[0].credit.start <= 4,
        world.nodes@[0].credit.earned == 0, world.nodes@[0].credit.added == 0,
        world.nodes@[0].credit.spent == 0,
    ensures world_headroom(world),
{
    reveal(world_headroom);
    reveal(initial_node);
    reveal_with_fuel(open_prefix, 2);
}

fn external(number: u64) -> (target: DependencyTarget)
    ensures target == (DependencyTarget { kind: PrerequisiteKind::Issue, number, request: None }),
{
    DependencyTarget { kind: PrerequisiteKind::Issue, number, request: None }
}

/// Covers "an unstarted root accepts a declaration" and "a started request
/// rejects a declaration" through the arbitrary-world registration theorem.
fn registration_covers() -> (result: (World, World))
    ensures result.0.dependencies@.len() == 2, result.1.dependencies@.len() == 1,
        result.1.nodes@[0].phase == (Phase::Planning { must_split: false }),
{
    let waiting = dependency_retirement::waiting_world();
    fixture_accepted(&waiting);
    let ghost w = waiting;
    let target = external(2762);
    proof {
        assert(found(w, 0, target, None));
        assert(writes_row(w, 0, target, DependencyStatus::Pending, None));
    }
    let (accepted, _) = registration(waiting, 0, target, DependencyStatus::Pending);
    let (started, _) = dependency_retirement::verify_abandoned(DependencyStatus::Delivered);
    fixture_accepted(&started);
    let ghost s = started;
    proof {
        assert(found(s, 0, target, None));
        assert(!writes_row(s, 0, target, DependencyStatus::Pending, None));
    }
    let (refused, _) = registration(started, 0, target, DependencyStatus::Pending);
    proof { reveal(dependency_frame); }
    (accepted, refused)
}

/// Two Ingesting roots in one repository, optionally with the edge 1->0.
fn graph_world(edge: bool) -> (world: World)
    ensures edges_indexable(world), acyclic(world), world.nodes@.len() == 2,
        world.dependencies@.len() == if edge { 1int } else { 0int },
        edge ==> world.dependencies@[0] == (Dependency { node: NodeId(1),
            target: DependencyTarget { kind: PrerequisiteKind::Issue, number: 1,
                request: Some(NodeId(0)) },
            status: DependencyStatus::Pending, rejected: false }),
        forall|j: int| 0 <= j < 2 ==> (#[trigger] world.nodes@[j]).phase == Phase::Ingesting
            && !world.nodes@[j].planning_started && world.nodes@[j].repo == RepoId(0),
{
    let mut a = Node::new(RepoId(0));
    a.planning_started = false;
    let mut b = Node::new(RepoId(0));
    b.planning_started = false;
    let mut nodes = Vec::new();
    nodes.push(a);
    nodes.push(b);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    let mut dependencies = Vec::new();
    if edge {
        dependencies.push(Dependency { node: NodeId(1), target: DependencyTarget {
            kind: PrerequisiteKind::Issue, number: 1, request: Some(NodeId(0)) },
            status: DependencyStatus::Pending, rejected: false });
    }
    let world = World { nodes, gates, dependencies, pool_capacity: 1, next_ticket: 0,
        next_question: 0, file_sets: Vec::new(), fail_fast: false };
    proof {
        reveal(initial_node);
        reveal(edges_indexable);
        acyclic_definition(world);
        assert forall|x: NodeId, y: NodeId| active_edge(world, x, y)
            implies !reachable(world, y, x) by {
            reveal(active_edge);
            let cut = vstd::iset::ISet::new(|n: NodeId| n == NodeId(0));
            assert forall|m: NodeId, n: NodeId| cut.contains(m) && active_edge(world, m, n)
                implies cut.contains(n) by { reveal(active_edge); }
            lemmas::cut_excludes(world, NodeId(0), NodeId(1), cut);
        }
    }
    world
}

/// Covers "a cyclic declaration is rejected" and "an acyclic declaration is
/// accepted" through the arbitrary-world no-cycle theorem.
fn no_cycle_covers() -> (result: (World, World))
    ensures result.0.dependencies@.len() == 2, result.0.dependencies@[1].rejected,
        result.1.dependencies@.len() == 1, !result.1.dependencies@[0].rejected,
        acyclic(result.0), acyclic(result.1),
{
    let target = DependencyTarget { kind: PrerequisiteKind::Issue, number: 2,
        request: Some(NodeId(1)) };
    let cyclic = graph_world(true);
    let ghost c = cyclic;
    proof {
        assert(found(c, 0, target, None));
        assert(target_ok(c, 0, target));
        assert(writes_row(c, 0, target, DependencyStatus::Pending, None));
        lemmas::reflexive(c, NodeId(1));
        reveal(active_edge);
        assert(active_edge(c, NodeId(1), NodeId(0)));
        lemmas::extend(c, NodeId(1), NodeId(1), NodeId(0));
    }
    let (rejected, _) = no_cycle(cyclic, 0, target, DependencyStatus::Pending);
    let open = graph_world(false);
    let ghost o = open;
    proof {
        assert(found(o, 0, target, None));
        assert(target_ok(o, 0, target));
        assert(writes_row(o, 0, target, DependencyStatus::Pending, None));
        let cut = vstd::iset::ISet::new(|n: NodeId| n == NodeId(1));
        assert forall|m: NodeId, n: NodeId| cut.contains(m) && active_edge(o, m, n)
            implies cut.contains(n) by { reveal(active_edge); }
        lemmas::cut_excludes(o, NodeId(1), NodeId(0), cut);
    }
    let (accepted, _) = no_cycle(open, 0, target, DependencyStatus::Pending);
    (rejected, accepted)
}

/// The original fixture with its root still ingesting and credit not yet ready.
fn ingesting_world() -> (world: World)
    ensures dependency_retirement::domain(world), dependency_retirement::accepted(world),
        world.nodes@[0].phase == Phase::Ingesting, !world.nodes@[0].planning_started,
        world.dependencies@[0].status == DependencyStatus::Pending,
{
    let mut node = Node::new(RepoId(0));
    node.planning_started = false;
    let mut nodes = Vec::new();
    nodes.push(node);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    let mut dependencies = Vec::new();
    dependencies.push(Dependency { node: NodeId(0), target: external(2761),
        status: DependencyStatus::Pending, rejected: false });
    let world = World { nodes, gates, dependencies, pool_capacity: 1, next_ticket: 0,
        next_question: 0, file_sets: Vec::new(), fail_fast: false };
    proof {
        reveal(initial_node);
        reveal(retirement::node_rules);
        reveal(zero_credit);
        footprint::inline_is_valid(world, 0);
    }
    world
}

/// Covers "re-ingest removes a changed declaration", "abandoned never reverts
/// during re-ingest" and "other phases preserve declarations".
fn removal_covers() -> (result: (World, World))
    ensures result.0.dependencies@.len() == 1,
        result.0.dependencies@[0].target.number == 2762,
        result.0.dependencies@[0].status == DependencyStatus::Abandoned,
        result.1.dependencies@.len() == 1,
{
    let world = ingesting_world();
    fixture_accepted(&world);
    let ghost w0 = world;
    let second = external(2762);
    proof {
        assert(found(w0, 0, second, None));
        assert(writes_row(w0, 0, second, DependencyStatus::Pending, None));
    }
    let (world, _) = registration(world, 0, second, DependencyStatus::Pending);
    let ghost w1 = world;
    proof {
        reveal(dependency_frame);
        assert(found(w1, 0, second, Some(1usize)));
        assert(writes_row(w1, 0, second, DependencyStatus::Abandoned, Some(1usize)));
    }
    let (world, _) = registration(world, 0, second, DependencyStatus::Abandoned);
    let ghost w2 = world;
    proof { reveal(dependency_frame); }
    let removed = removal(world, 0, PrerequisiteKind::Issue, 2761);
    proof {
        reveal_with_fuel(kept, 3);
        let s = w2.dependencies@;
        assert(s.drop_last().drop_last() =~= Seq::<Dependency>::empty());
        assert(s.drop_last() =~= seq![s[0]]);
    }
    let waiting = dependency_retirement::waiting_world();
    fixture_accepted(&waiting);
    let kept_all = removal(waiting, 0, PrerequisiteKind::Issue, 2761);
    (removed, kept_all)
}

/// Covers "an open prerequisite blocks planning" with the original empty
/// footprint, zero balance and empty output.
fn waiting_cover() -> (result: (World, bool, Outbox, i128))
    ensures !result.1, result.2.len == 0, result.3 == 0,
{
    let world = dependency_retirement::waiting_world();
    fixture_accepted(&world);
    proof {
        fixture_reserves(world);
        pending_blocks(world, 0);
        prerequisite_definition(world, NodeId(0));
    }
    let ghost w = world;
    let result = waiting_holds_nothing(world, 0);
    let empty = result.0.nodes[0].footprint.is_empty();
    proof {
        reveal(initial_node);
        footprint::inline_is_valid(w, 0);
    }
    assert(empty);
    result
}

/// Covers "delivery admits planning", with the strict measure decrease and
/// the original released balance of 4.
fn delivered_cover() -> (result: (World, bool, i128))
    ensures result.1, result.0.nodes@[0].planning_started, result.2 == 4,
        result.0.nodes@[0].phase == (Phase::Planning { must_split: false }),
{
    let world = dependency_retirement::waiting_world();
    fixture_accepted(&world);
    let target = external(2761);
    proof {
        fixture_reserves(world);
        checking::accepted_parts(world);
        rules_checked(world);
        assert(world.dependencies@[0].target == target);
        pending_ready(world, 0, target);
    }
    let ghost w = world;
    let delivered_world = delivered_releases(world, 0, target);
    proof {
        reveal(dependency_frame);
        reveal(delivery_changed);
        reveal(initial_node);
        delivered_definition(delivered_world.dependencies@, NodeId(0));
        reveal_with_fuel(potential_prefix, 2);
    }
    let (admitted, moved) = delivered_admits(delivered_world, 0);
    let balance = admitted.nodes[0].credit.balance();
    (admitted, moved, balance)
}

// ------------------------------------------------- single-node updates (K4c)

/// One node changes and the table does not: rules_ok is preserved when the
/// owner has no undelivered prerequisite, or still waits with every waiting-owner
/// condition, and withheld credit is newly set only while it waits.
pub(super) proof fn node_update_preserves(before: World, after: World, i: int)
    requires rules_ok(before), 0 <= i < before.nodes@.len(), before.nodes@.len() <= u64::MAX,
        after.nodes@ == before.nodes@.update(i, after.nodes@[i]),
        after.dependencies@ == before.dependencies@,
        after.nodes@[i].repo == before.nodes@[i].repo,
        before.nodes@[i].phase is Closed ==> after.nodes@[i].phase is Closed,
        after.nodes@[i].credit.withheld ==> before.nodes@[i].credit.withheld
            || !prerequisites_delivered(before, NodeId(i as u64)),
        prerequisites_delivered(before, NodeId(i as u64)) || owner_waiting_ok(after.nodes@[i]),
    ensures rules_ok(after),
{
    rules_definition(before);
    rules_definition(after);
    let len = before.nodes@.len();
    let owner = NodeId(i as u64);
    assert(after.nodes@.len() == len);
    assert forall|j: int| 0 <= j < len && j != i implies #[trigger] after.nodes@[j] == before.nodes@[j] by {
        assert(after.nodes@[j] == before.nodes@.update(i, after.nodes@[i])[j]);
    }
    assert forall|o: NodeId| prerequisites_delivered(after, o) == prerequisites_delivered(before, o) by {
        prerequisite_definition(after, o);
        prerequisite_definition(before, o);
    }
    prerequisite_definition(before, owner);
    delivered_definition(before.dependencies@, owner);
    assert forall|k: int| 0 <= k < after.dependencies@.len()
        implies row_ok(after, #[trigger] after.dependencies@[k]) by {
        let d = after.dependencies@[k];
        assert(row_ok(before, before.dependencies@[k]));
        if d.node.0 < len {
            if d.node.0 as int == i {
                assert(d.node == owner);
            } else {
                assert(after.nodes@[d.node.0 as int] == before.nodes@[d.node.0 as int]);
            }
        }
        if let Some(t) = d.target.request {
            if t.0 < len && t.0 as int != i {
                assert(after.nodes@[t.0 as int] == before.nodes@[t.0 as int]);
            }
        }
    }
    assert forall|j: int| 0 <= j < after.nodes@.len() && #[trigger] after.nodes@[j].credit.withheld
        implies exists|k: int| 0 <= k < after.dependencies@.len()
            && after.dependencies@[k].node == NodeId(j as u64) by {
        if j == i && !before.nodes@[i].credit.withheld {
            let k = choose|k: int| 0 <= k < before.dependencies@.len()
                && before.dependencies@[k].node == owner
                && !(!before.dependencies@[k].rejected
                    && before.dependencies@[k].status == DependencyStatus::Delivered);
        } else {
            assert(before.nodes@[j].credit.withheld);
        }
    }
    assert forall|a: NodeId, b: NodeId| #[trigger] active_edge(after, a, b)
        implies active_edge(before, a, b) by {
        reveal(active_edge);
        if a.0 as int != i {
            assert(after.nodes@[a.0 as int] == before.nodes@[a.0 as int]);
        }
    }
    acyclic_after(before, after, NodeId(0), NodeId(0), false);
}

// ------------------------------------------------------ node appends (K5 G19)

/// Appended nodes own no row and hold no withheld credit; the old nodes keep
/// every endpoint field the checker reads. Then rules_ok is preserved.
pub(super) proof fn append_preserves(before: World, after: World)
    requires rules_ok(before),
        after.dependencies@ == before.dependencies@,
        after.nodes@.len() >= before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len()
            ==> same_endpoint(#[trigger] before.nodes@[j], after.nodes@[j]),
        forall|j: int| before.nodes@.len() <= j < after.nodes@.len()
            ==> (#[trigger] after.nodes@[j]) == after.nodes@[j],
    ensures rules_ok(after),
{
    rules_definition(before);
    rules_definition(after);
    let len = before.nodes@.len();
    assert forall|o: NodeId| prerequisites_delivered(after, o) == prerequisites_delivered(before, o) by {
        prerequisite_definition(after, o);
        prerequisite_definition(before, o);
    }
    assert forall|k: int| 0 <= k < after.dependencies@.len()
        implies row_ok(after, #[trigger] after.dependencies@[k]) by {
        let d = after.dependencies@[k];
        assert(row_ok(before, before.dependencies@[k]));
        assert(same_endpoint(before.nodes@[d.node.0 as int], after.nodes@[d.node.0 as int]));
        if let Some(t) = d.target.request {
            assert(same_endpoint(before.nodes@[t.0 as int], after.nodes@[t.0 as int]));
        }
    }
    assert forall|j: int| 0 <= j < after.nodes@.len() && #[trigger] after.nodes@[j].credit.withheld
        implies exists|k: int| 0 <= k < after.dependencies@.len()
            && after.dependencies@[k].node == NodeId(j as u64) by {
        if j < len {
            assert(same_endpoint(before.nodes@[j], after.nodes@[j]));
            assert(before.nodes@[j].credit.withheld);
        }
    }
    // Every edge of `after` is an edge of `before` between old nodes.
    assert forall|a: NodeId, b: NodeId| #[trigger] active_edge(after, a, b)
        implies active_edge(before, a, b) by {
        reveal(active_edge);
        let k = choose|k: int| 0 <= k < after.dependencies@.len()
            && after.dependencies@[k].node == a && after.dependencies@[k].target.request == Some(b)
            && !after.dependencies@[k].rejected
            && after.dependencies@[k].status == DependencyStatus::Pending;
        assert(row_ok(before, before.dependencies@[k]));
        assert(same_endpoint(before.nodes@[a.0 as int], after.nodes@[a.0 as int]));
    }
    acyclic_definition(before);
    acyclic_definition(after);
    // A cycle route in `after` visits only old nodes, so it is a route in `before`.
    assert forall|a: NodeId, b: NodeId| active_edge(after, a, b) implies !reachable(after, b, a) by {
        if reachable(after, b, a) {
            reveal(reachable);
            let route = choose|route: Seq<NodeId>| path(after, route)
                && route[0] == b && route[route.len() - 1] == a;
            reveal(path);
            assert(active_edge(before, a, b));
            assert forall|x: int| 0 <= x < route.len() - 1
                implies active_edge(before, #[trigger] route[x], route[x + 1]) by {
                assert(active_edge(after, route[x], route[x + 1]));
            }
            assert forall|x: int| 0 <= x < route.len() implies route[x].0 < len by {
                reveal(active_edge);
                if x < route.len() - 1 {
                    assert(active_edge(before, route[x], route[x + 1]));
                } else {
                    assert(route[x] == a);
                }
            }
            assert(path(before, route));
            assert(reachable(before, b, a));
        }
    }
}

/// Covers K5 G19 append: the waiting fixture plus one fresh Ingesting request.
fn append_cover() -> (result: World)
    ensures rules_ok(result), result.nodes@.len() == 2,
        result.nodes@[0].credit.withheld, result.nodes@[1].phase == Phase::Ingesting,
        !result.nodes@[1].credit.withheld,
{
    let mut world = dependency_retirement::waiting_world();
    fixture_accepted(&world);
    let ghost before = world;
    world.nodes.push(Node::new(RepoId(0)));
    proof {
        checking::accepted_parts(before);
        reveal(initial_node);
        assert forall|j: int| 0 <= j < before.nodes@.len()
            implies same_endpoint(#[trigger] before.nodes@[j], world.nodes@[j]) by {
            assert(world.nodes@[j] == before.nodes@[j]);
        }
        append_preserves(before, world);
    }
    world
}
}
