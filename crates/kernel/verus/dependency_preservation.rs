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

/// The dependency handlers keep every non-dependency checker rule.
pub(super) proof fn checker_frame(before: World, after: World, i: int)
    requires progress_gate_apply::accepted(before), dependency_frame(before, after, i),
        0 <= i < before.nodes@.len(),
        after.nodes@[i].credit.withheld ==> before.nodes@[i].credit.withheld
            || (before.nodes@[i].credit.ready && matches!(before.nodes@[i].phase,
                Phase::Ingesting | Phase::Auditing | Phase::Unplanned)),
    ensures progress_gate_apply::accepted(after),
{
    reveal(dependency_frame);
    reveal(progress_gate_apply::accepted);
    let len = before.nodes@.len();
    assert forall|j: int| 0 <= j < len implies (#[trigger] after.nodes@[j]).holds == before.nodes@[j].holds
        && after.nodes@[j].repo == before.nodes@[j].repo && after.nodes@[j].pool == before.nodes@[j].pool
        && after.nodes@[j].footprint == before.nodes@[j].footprint
        && after.nodes@[j].ticket == before.nodes@[j].ticket by {}
    assert forall|r: int| 0 <= r < after.gates@.len() implies after.gates@[r].w > 0
        && progress_gate_apply::holders(after, RepoId(r as u64), after.nodes@.len()) <= after.gates@[r].w by {
        holders_frame(before, after, RepoId(r as u64), len);
    }
    pool_frame(before.nodes@, after.nodes@, len);
    assert forall|a: int, b: int| 0 <= a < b < after.nodes@.len()
        implies progress_gate_apply::pair(after, a, b) by {
        footprint::storage_frame(before, after, before.nodes@[a].footprint, before.nodes@[b].footprint);
        assert(progress_gate_apply::pair(before, a, b));
    }
    assert forall|j: int| 0 <= j < after.nodes@.len()
        implies retirement::node_rules(after, after.nodes@[j]) by {
        retirement::node_frame(before, after, before.nodes@[j]);
        assert(retirement::node_rules(before, before.nodes@[j]));
        if j == i { owner_rules(after, before.nodes@[i], after.nodes@[i]); }
    }
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
        && after.nodes@[i].credit.withheld == withheld_after(before.nodes@[i],
            after.dependencies@, NodeId(i as u64))
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
    ensures (declared(before, i, target, status).rejected
            || declared(before, i, target, status).status != DependencyStatus::Delivered)
            ==> !before.nodes@[i].planning_started && waits(before.nodes@[i].question)
                && matches!(before.nodes@[i].phase, Phase::Ingesting | Phase::Auditing
                    | Phase::Unplanned | Phase::AwaitingRob | Phase::Stopping(_) | Phase::Closed(_)),
{
    let n = before.nodes@[i];
    match existing {
        None => { early_waits(before, n); }
        Some(k) => {
            let previous = before.dependencies@[k as int];
            assert(row_ok(before, previous));
            assert(previous.node == NodeId(i as u64));
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
            || (before.nodes@[i].credit.ready && matches!(before.nodes@[i].phase,
                Phase::Ingesting | Phase::Auditing | Phase::Unplanned)),
{
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
                && after.nodes@[i].credit.withheld == withheld_after(before.nodes@[i],
                    after.dependencies@, NodeId(i as u64))
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
            || (before.nodes@[i].credit.ready && matches!(before.nodes@[i].phase,
                Phase::Ingesting | Phase::Auditing | Phase::Unplanned)),
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
        assert forall|a: int| 0 <= a < t.len() implies row_ok(after, #[trigger] t[a]) by {
            let b = choose|b: int| 0 <= b < s.len() && t[a] == s[b] && !same_key(s[b], owner, kind, number);
            assert(row_ok(before, s[b]));
            prerequisite_definition(after, t[a].node);
            prerequisite_definition(before, t[a].node);
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
}
