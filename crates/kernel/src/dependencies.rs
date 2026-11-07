use super::*;

#[cfg(verus_keep_ghost)]
#[path = "../verus/dependencies.rs"]
pub(super) mod lemmas;

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dependency {
    pub node: NodeId,
    pub target: DependencyTarget,
    pub status: DependencyStatus,
    /// A rejected declaration blocks its owner without adding an edge to the graph.
    pub rejected: bool,
}

#[cfg(verus_keep_ghost)]
verus! {
/// An actual live declaration, independent of either traversal's storage.
#[verifier::opaque]
pub(super) open spec fn active_edge(w: World, from: NodeId, to: NodeId) -> bool {
    from.0 < w.nodes@.len() && to.0 < w.nodes@.len()
        && !(w.nodes@[from.0 as int].phase is Closed)
        && exists|i: int| 0 <= i < w.dependencies@.len()
            && w.dependencies@[i].node == from
            && w.dependencies@[i].target.request == Some(to)
            && !w.dependencies@[i].rejected
            && w.dependencies@[i].status == DependencyStatus::Pending
}

/// Finite paths include a singleton at any valid vertex, including Closed.
#[verifier::opaque]
pub(super) open spec fn path(w: World, route: Seq<NodeId>) -> bool {
    route.len() > 0
        && (forall|i: int| 0 <= i < route.len() ==> route[i].0 < w.nodes@.len())
        && (forall|i: int| 0 <= i < route.len() - 1
            ==> active_edge(w, #[trigger] route[i], route[i + 1]))
}

#[verifier::opaque]
pub(super) open spec fn reachable(w: World, from: NodeId, to: NodeId) -> bool {
    exists|route: Seq<NodeId>| path(w, route)
        && route[0] == from && route[route.len() - 1] == to
}

#[verifier::opaque]
pub closed spec fn acyclic(w: World) -> bool {
    forall|from: NodeId, to: NodeId| active_edge(w, from, to)
        ==> !reachable(w, to, from)
}

/// Closed vertices other than the sought endpoint need no expansion.
#[verifier::opaque]
pub(super) open spec fn explored(w: World, seen: Seq<bool>, to: NodeId,
    node: NodeId) -> bool {
    node.0 < w.nodes@.len() && (seen[node.0 as int]
        || ((w.nodes@[node.0 as int].phase is Closed) && node != to))
}

/// State-table planning admission / #693 / #771: every prerequisite is delivered.
pub closed spec fn delivered(dependencies: Seq<Dependency>, node: NodeId) -> bool {
    forall|i: int| 0 <= i < dependencies.len() && dependencies[i].node == node
        ==> !dependencies[i].rejected && dependencies[i].status == DependencyStatus::Delivered
}

/// #693 / #806: blocked credit is unavailable or has zero ledger balance.
#[verifier::opaque]
pub(super) open spec fn zero_credit(credit: Credit) -> bool {
    credit.withheld || credit.start as int + credit.earned as int + credit.added as int == credit.spent as int
}

/// #693 / #806: expose the existing universal delivery predicate to ported proofs.
pub(super) proof fn delivered_definition(edges: Seq<Dependency>, node: NodeId)
    ensures delivered(edges, node) == (forall|i: int| 0 <= i < edges.len() && edges[i].node == node
        ==> !edges[i].rejected && edges[i].status == DependencyStatus::Delivered),
{
    reveal(delivered);
}

/// #693 / #806: external declarations and rejected cycles have no active graph edge.
pub closed spec fn no_active_graph_edges(w: World) -> bool {
    forall|i: int| 0 <= i < w.dependencies@.len() ==>
        w.dependencies@[i].rejected || w.dependencies@[i].status != DependencyStatus::Pending
            || w.dependencies@[i].target.request.is_none()
}

/// #771: public ghost view of the private executable prerequisite table.
pub closed spec fn prerequisites_delivered(world: World, node: NodeId) -> bool {
    delivered(world.dependencies@, node)
}

/// #771: the public query depends only on the actual prerequisite table.
pub(super) proof fn prerequisite_definition(world: World, node: NodeId)
    ensures prerequisites_delivered(world, node) == delivered(world.dependencies@, node),
{
    reveal(prerequisites_delivered);
}

/// #693 / #771: declarations retained by the controller name existing roots.
pub closed spec fn edges_indexable(world: World) -> bool {
    forall|i: int| 0 <= i < world.dependencies@.len() ==>
        world.dependencies@[i].node.0 < world.nodes@.len()
        && match world.dependencies@[i].target.request {
            None => true, Some(node) => node.0 < world.nodes@.len(),
        }
}

/// #805: expose the existing owner/target index invariant across proof modules.
pub(super) proof fn indexable_definition(world: World)
    ensures edges_indexable(world) == (forall|i: int| 0 <= i < world.dependencies@.len() ==>
        world.dependencies@[i].node.0 < world.nodes@.len()
        && match world.dependencies@[i].target.request {
            None => true, Some(node) => node.0 < world.nodes@.len(),
        }),
{
    reveal(edges_indexable);
}

/// #693 / #771: one declaration's existing consistency requirements.
pub(super) open spec fn dependency_checked(world: World, dependency: Dependency) -> bool {
    let owner = world.nodes@[dependency.node.0 as int];
    dependency.node.0 < world.nodes@.len() && dependency.target.number > 0
        && (match dependency.target.request { None => true, Some(n) => n.0 < world.nodes@.len()
            && world.nodes@[n.0 as int].repo == owner.repo })
        && ((dependency.rejected || dependency.status != DependencyStatus::Delivered) ==>
            !world.nodes@[dependency.node.0 as int].planning_started
                && matches!(owner.phase,
                    Phase::Ingesting | Phase::Auditing | Phase::Unplanned | Phase::AwaitingRob
                        | Phase::Stopping(_) | Phase::Closed(_)))
        && (world.nodes@[dependency.node.0 as int].phase == Phase::Unplanned
            && !prerequisites_delivered(world, dependency.node) ==>
                !world.nodes@[dependency.node.0 as int].holds && !world.nodes@[dependency.node.0 as int].pool
                    && world.nodes@[dependency.node.0 as int].ticket.is_none()
                    && world.nodes@[dependency.node.0 as int].run.is_none()
                    && zero_credit(world.nodes@[dependency.node.0 as int].credit))
}

/// #693 / #771: executable prerequisite checks imply this progress projection.
pub(super) open spec fn dependencies_checked(world: World) -> bool {
    edges_indexable(world) && declarations_unique(world)
        && forall|j: int| 0 <= j < world.dependencies@.len()
            ==> dependency_checked(world, world.dependencies@[j])
}

pub(super) open spec fn same_declaration(a: Dependency, b: Dependency) -> bool {
    a.node == b.node && a.target.kind == b.target.kind && a.target.number == b.target.number
}

pub(super) open spec fn declarations_unique(w: World) -> bool {
    forall|a: int, b: int| 0 <= a < b < w.dependencies@.len()
        ==> !same_declaration(w.dependencies@[a], w.dependencies@[b])
}

/// #693 / #805: the production checker's key uniqueness supplies delivery guards.
pub(super) proof fn pending_ready(w: World, i: int, target: DependencyTarget)
    requires dependencies_checked(w), 0 <= i < w.nodes@.len(), i <= u64::MAX as int,
        !(w.nodes@[i].phase is Closed),
        exists|k: int| 0 <= k < w.dependencies@.len()
            && w.dependencies@[k].node == NodeId(i as u64) && w.dependencies@[k].target == target
            && !w.dependencies@[k].rejected && w.dependencies@[k].status == DependencyStatus::Pending,
    ensures delivery_input(w, i, target),
{
    reveal(delivery_input);
    let k = choose|k: int| 0 <= k < w.dependencies@.len()
        && w.dependencies@[k].node == NodeId(i as u64) && w.dependencies@[k].target == target
        && !w.dependencies@[k].rejected && w.dependencies@[k].status == DependencyStatus::Pending;
    assert forall|j: int| 0 <= j < w.dependencies@.len()
        && w.dependencies@[j].node == NodeId(i as u64)
        && w.dependencies@[j].target.kind == target.kind && w.dependencies@[j].target.number == target.number
        implies w.dependencies@[j].target == target && !w.dependencies@[j].rejected
            && w.dependencies@[j].status == DependencyStatus::Pending by {
        if j < k { assert(!same_declaration(w.dependencies@[j], w.dependencies@[k])); }
        if k < j { assert(!same_declaration(w.dependencies@[k], w.dependencies@[j])); }
        assert(j == k);
    }
}

/// State-table prerequisite wait / #805: a concrete Pending entry blocks admission.
pub(super) proof fn pending_blocks(w: World, k: int)
    requires 0 <= k < w.dependencies@.len(),
        w.dependencies@[k].status == DependencyStatus::Pending,
    ensures !delivered(w.dependencies@, w.dependencies@[k].node),
{
    reveal(delivered);
}

/// #805: immutable declarations and storage length preserve index validity.
pub(super) proof fn indexability_frame(before: World, after: World)
    requires edges_indexable(before), after.dependencies@ == before.dependencies@,
        after.nodes@.len() == before.nodes@.len(),
    ensures edges_indexable(after),
{
    reveal(edges_indexable);
}

/// Prerequisite fact rows / #693 / #771: exact ledger and node frame shared
/// by declaration composition and its table update. Opacity keeps unrelated
/// dependency quantifiers out of the wrapper's verification context.
#[verifier::opaque]
pub(super) open spec fn dependency_frame(before: World, after: World, i: int) -> bool {
    after.nodes@[i] == (Node {
            waiting_prerequisite: after.nodes@[i].waiting_prerequisite,
            credit: Credit { withheld: after.nodes@[i].credit.withheld,
                ..before.nodes@[i].credit },
            ..before.nodes@[i]
        })
            && after.next_ticket == before.next_ticket
            && after.next_question == before.next_question
            && after.gates@ == before.gates@
            && after.pool_capacity == before.pool_capacity
            && after.fail_fast == before.fail_fast
            && after.file_sets@ == before.file_sets@
            && after.nodes@.len() == before.nodes@.len()
            && (forall|j: int| 0 <= j < before.nodes@.len() && j != i
                ==> after.nodes@[j] == before.nodes@[j])
            && after.nodes@[i as int].repo == before.nodes@[i as int].repo
            && after.nodes@[i as int].footprint == before.nodes@[i as int].footprint
            && after.nodes@[i as int].phase == before.nodes@[i as int].phase
            && after.nodes@[i as int].pos == before.nodes@[i as int].pos
            && after.nodes@[i as int].pool == before.nodes@[i as int].pool
            && after.nodes@[i as int].holds == before.nodes@[i as int].holds
            && after.nodes@[i as int].last_cmd == before.nodes@[i as int].last_cmd
            && after.nodes@[i as int].generation == before.nodes@[i as int].generation
            && after.nodes@[i as int].next_seq == before.nodes@[i as int].next_seq
            && after.nodes@[i as int].credit.start == before.nodes@[i as int].credit.start
            && after.nodes@[i as int].credit.earned == before.nodes@[i as int].credit.earned
            && after.nodes@[i as int].credit.added == before.nodes@[i as int].credit.added
            && after.nodes@[i as int].credit.spent == before.nodes@[i as int].credit.spent
}

/// Prerequisite delivery / #805: the real lookup has a pending row and every
/// matching declaration passes the handler's guards. check_dependencies rejects
/// duplicate keys; no world-size bound is imposed here.
#[verifier::opaque]
pub(super) open spec fn delivery_input(w: World, i: int, target: DependencyTarget) -> bool {
    !(w.nodes@[i].phase is Closed) && target.number > 0
        && (match target.request { None => true, Some(node) => node.0 < w.nodes@.len()
            && w.nodes@[node.0 as int].repo == w.nodes@[i].repo })
        && (exists|k: int| 0 <= k < w.dependencies@.len()
            && w.dependencies@[k].node == NodeId(i as u64) && w.dependencies@[k].target == target
            && !w.dependencies@[k].rejected && w.dependencies@[k].status == DependencyStatus::Pending)
        && (forall|k: int| 0 <= k < w.dependencies@.len()
            && w.dependencies@[k].node == NodeId(i as u64)
            && w.dependencies@[k].target.kind == target.kind && w.dependencies@[k].target.number == target.number
            ==> w.dependencies@[k].target == target && !w.dependencies@[k].rejected
                && w.dependencies@[k].status == DependencyStatus::Pending)
}

/// #805: expose the concrete changed row across the actual handler boundary.
#[verifier::opaque]
pub(super) open spec fn delivery_changed(before: World, after: World, i: int,
    target: DependencyTarget) -> bool {
    exists|k: int| 0 <= k < before.dependencies@.len()
        && before.dependencies@[k].node == NodeId(i as u64)
        && before.dependencies@[k].target == target
        && before.dependencies@[k].status == DependencyStatus::Pending
        && after.dependencies@ == before.dependencies@.update(k, Dependency {
            status: DependencyStatus::Delivered, rejected: false, ..before.dependencies@[k] })
}

pub closed spec fn unseen_prefix(seen: Seq<bool>, end: nat) -> nat
    recommends end <= seen.len(),
    decreases end,
{
    if end == 0 { 0 } else { unseen_prefix(seen, (end - 1) as nat)
        + if seen[end as int - 1] { 0nat } else { 1nat } }
}

/// #771: counting a prefix depends only on that prefix's elements.
pub proof fn unseen_prefix_equal(left: Seq<bool>, right: Seq<bool>, end: nat)
    requires end <= left.len(), end <= right.len(),
        forall|i: int| 0 <= i < end ==> left[i] == right[i],
    ensures unseen_prefix(left, end) == unseen_prefix(right, end),
    decreases end,
{
    reveal(unseen_prefix);
    if end > 0 { unseen_prefix_equal(left, right, (end - 1) as nat); }
}

/// #771: visiting an unvisited root pays for at most one complete edge scan.
pub proof fn visit_reduces_unseen(seen: Seq<bool>, index: int, end: nat)
    requires 0 <= index < end <= seen.len(), !seen[index],
    ensures unseen_prefix(seen.update(index, true), end) + 1 == unseen_prefix(seen, end),
    decreases end,
{
    reveal(unseen_prefix);
    if index < end - 1 {
        visit_reduces_unseen(seen, index, (end - 1) as nat);
    } else {
        unseen_prefix_equal(seen.update(index, true), seen, (end - 1) as nat);
    }
}

}

kernel_fn! {
/// #839 / Q27: finite total decision over 3 statuses x 2 rejection states x
/// 3 incoming statuses x 2 revocation states. No loop or unwind. Reopens revoke
/// provisional delivery; consumed admission and rejected cycles stay unchanged.
fn update_allowed(previous: DependencyStatus, rejected: bool, incoming: DependencyStatus,
    revocable: bool) -> (allowed: bool) [
    ensures allowed == (!rejected && (previous == DependencyStatus::Pending
        || (previous == DependencyStatus::Abandoned && incoming == DependencyStatus::Delivered)
        || (revocable && incoming == DependencyStatus::Pending))),
] {
    !rejected && match previous {
        DependencyStatus::Pending => true,
        DependencyStatus::Delivered => revocable && incoming == DependencyStatus::Pending,
        DependencyStatus::Abandoned => incoming == DependencyStatus::Delivered
            || (revocable && incoming == DependencyStatus::Pending),
    }
}
}

impl World {
    pub fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }

    kernel_fn! {
    /// State-table planning admission / #771: check every prerequisite declaration.
    pub fn dependencies_delivered(&self, node: NodeId) -> (result: bool) [
        ensures result == prerequisites_delivered(*self, node),
    ] proof [reveal(prerequisites_delivered); reveal(delivered);] {
        let mut i = 0usize;
        kernel_while! { (i < self.dependencies.len()) [
            invariant i <= self.dependencies.len(),
                forall|j: int| 0 <= j < i && self.dependencies@[j].node == node
                    ==> !self.dependencies@[j].rejected
                        && self.dependencies@[j].status == DependencyStatus::Delivered,
            decreases self.dependencies.len() - i,
        ] {
            let d = &self.dependencies[i];
            if d.node == node && (d.rejected || d.status != DependencyStatus::Delivered) {
                return false;
            }
            i += 1;
        } }
        true
    }
    }

    kernel_fn! {
    /// #693 / #771: both production cycle-detection branches terminate. Bitset
    /// relaxation strictly raises a u64 mask; the large traversal charges each
    /// push to one edge of a newly visited root. Neither branch writes a ledger.
    #[allow(clippy::collapsible_if)]
    fn dependency_path(&self, from: NodeId, to: NodeId) -> (result: bool) [
        requires from.0 < self.nodes@.len(), to.0 < self.nodes@.len(),
            edges_indexable(*self),
        ensures result == reachable(*self, from, to),
    ] proof [reveal(edges_indexable); reveal(active_edge); reveal(explored);] {
        kernel_proof! { lemmas::reflexive(*self, from); }
        if self.nodes.len() <= 64 {
            // Most worlds fit in a bitset. Repeated edge relaxation reaches
            // the same nodes as the unbounded traversal below, without allocation.
            let mut seen = 1u64 << from.0;
            kernel_proof! {
                assert forall|n: NodeId| n.0 < self.nodes@.len()
                    implies (seen & (1u64 << n.0) != 0) == (n == from) by {
                    lemmas::singleton_bit(from.0, n.0);
                }
            }
            kernel_loop! { [
                invariant self.nodes.len() <= 64, from.0 < self.nodes@.len(),
                    to.0 < self.nodes@.len(), edges_indexable(*self),
                    seen & (1u64 << from.0) != 0,
                    forall|n: NodeId| n.0 < self.nodes@.len() && seen & (1u64 << n.0) != 0
                        ==> reachable(*self, from, n),
                decreases u64::MAX as int - seen as int,
            ] {
                if seen & (1u64 << to.0) != 0 {
                    return true;
                }
                let before = seen;
                let mut index = 0usize;
                kernel_while! { (index < self.dependencies.len()) [
                    invariant index <= self.dependencies.len(), seen >= before,
                        self.nodes.len() <= 64, edges_indexable(*self),
                        from.0 < self.nodes@.len(), to.0 < self.nodes@.len(),
                        seen & (1u64 << from.0) != 0,
                        forall|n: NodeId| n.0 < self.nodes@.len()
                            && before & (1u64 << n.0) != 0 ==> seen & (1u64 << n.0) != 0,
                        forall|n: NodeId| n.0 < self.nodes@.len() && seen & (1u64 << n.0) != 0
                            ==> reachable(*self, from, n),
                        forall|j: int| 0 <= j < index
                            && !self.dependencies@[j].rejected
                            && self.dependencies@[j].status == DependencyStatus::Pending
                            && !(self.nodes@[self.dependencies@[j].node.0 as int].phase is Closed)
                            && before & (1u64 << self.dependencies@[j].node.0) != 0
                            && self.dependencies@[j].target.request.is_some()
                            ==> seen & (1u64 << self.dependencies@[j].target.request.unwrap().0) != 0,
                    decreases self.dependencies.len() - index,
                ] {
                    let dependency = self.dependencies[index];
                    if !dependency.rejected
                        && dependency.status == DependencyStatus::Pending
                        && !self.nodes[dependency.node.0 as usize].phase.is_closed()
                        && seen & (1u64 << dependency.node.0) != 0
                    {
                        if let Some(target) = dependency.target.request {
                            let mask = 1u64 << target.0;
                            kernel_proof! {
                                reveal(active_edge);
                                assert(active_edge(*self, dependency.node, target));
                                lemmas::extend(*self, from, dependency.node, target);
                                assert forall|n: NodeId| n.0 < self.nodes@.len()
                                    implies ((seen | mask) & (1u64 << n.0) != 0)
                                        == (seen & (1u64 << n.0) != 0 || n == target) by {
                                    lemmas::union_bit(seen, target.0, n.0);
                                }
                                assert((seen | mask) >= seen) by(bit_vector);
                            }
                            seen |= mask;
                        }
                    }
                    index += 1;
                } }
                if seen == before {
                    kernel_proof! {
                        let cut = vstd::iset::ISet::new(|n: NodeId| n.0 < self.nodes@.len()
                            && seen & (1u64 << n.0) != 0);
                        assert forall|a: NodeId, b: NodeId| cut.contains(a)
                            && active_edge(*self, a, b) implies cut.contains(b) by {
                            reveal(active_edge);
                            let j = choose|j: int| 0 <= j < self.dependencies@.len()
                                && self.dependencies@[j].node == a
                                && self.dependencies@[j].target.request == Some(b)
                                && !self.dependencies@[j].rejected
                                && self.dependencies@[j].status == DependencyStatus::Pending;
                            assert(seen & (1u64 << b.0) != 0);
                        }
                        lemmas::cut_excludes(*self, from, to, cut);
                    }
                    return false;
                }
            } }
        }
        #[cfg(kani)]
        {
            // Proof worlds have at most three roots. Prove this before pruning
            // the heap traversal, as for interned footprints.
            assert!(false, "dependency proof entered the large-world traversal");
            kani::assume(false);
        }
        let mut seen = alloc::vec![false; self.nodes.len()];
        let mut pending = alloc::vec![from];
        kernel_loop! { [
            invariant seen.len() == self.nodes.len(), edges_indexable(*self),
                from.0 < self.nodes@.len(), to.0 < self.nodes@.len(),
                !seen@[to.0 as int],
                forall|n: NodeId| n.0 < seen.len() && seen@[n.0 as int]
                    ==> reachable(*self, from, n),
                forall|i: int| 0 <= i < pending@.len() ==> pending@[i].0 < self.nodes@.len()
                    && reachable(*self, from, pending@[i]),
                explored(*self, seen@, to, from) || pending@.contains(from),
                forall|a: NodeId, b: NodeId| explored(*self, seen@, to, a)
                    && active_edge(*self, a, b)
                    ==> explored(*self, seen@, to, b) || pending@.contains(b),
            decreases unseen_prefix(seen@, seen@.len()) * (self.dependencies@.len() + 1)
                + pending@.len(),
        ] {
            kernel_snapshot! { before_metric = unseen_prefix(seen@, seen@.len())
                * (self.dependencies@.len() + 1) + pending@.len() }
            kernel_snapshot! { before_unseen = unseen_prefix(seen@, seen@.len()) }
            kernel_snapshot! { before_pending = pending@.len() }
            kernel_snapshot! { frontier_before = pending@ }
            let Some(node) = pending.pop() else {
                kernel_proof! {
                    reveal(explored);
                    let cut = vstd::iset::ISet::new(|n: NodeId| explored(*self, seen@, to, n));
                    lemmas::cut_excludes(*self, from, to, cut);
                }
                return false;
            };
            kernel_proof! {
                assert(frontier_before@[frontier_before@.len() - 1] == node);
                assert(node.0 < self.nodes@.len());
                assert(reachable(*self, from, node));
                assert forall|n: NodeId| frontier_before@.contains(n) && n != node
                    implies pending@.contains(n) by {
                    let j = choose|j: int| 0 <= j < frontier_before@.len()
                        && frontier_before@[j] == n;
                    assert(j < pending@.len());
                    assert(pending@[j] == n);
                }
            }
            if node == to {
                return true;
            }
            let Some(index) = self.index(node) else {
                continue;
            };
            if seen[index] || self.nodes[index].phase.is_closed() {
                kernel_proof! {
                    reveal(explored);
                    assert(explored(*self, seen@, to, node));
                    assert forall|a: NodeId, b: NodeId| explored(*self, seen@, to, a)
                        && active_edge(*self, a, b)
                        implies explored(*self, seen@, to, b) || pending@.contains(b) by {
                        if !explored(*self, seen@, to, b) {
                            assert(b != node);
                            assert(frontier_before@.contains(b));
                        }
                    }
                }
                continue;
            }
            kernel_proof! { visit_reduces_unseen(seen@, index as int, seen@.len()); }
            seen[index] = true;
            kernel_proof! {
                reveal(explored);
                assert(explored(*self, seen@, to, from) || pending@.contains(from));
                assert forall|a: NodeId, b: NodeId| a != node && explored(*self, seen@, to, a)
                    && active_edge(*self, a, b)
                    implies explored(*self, seen@, to, b) || pending@.contains(b) by {
                    if !explored(*self, seen@, to, b) {
                        assert(b != node);
                        assert(frontier_before@.contains(b));
                    }
                }
                assert(unseen_prefix(seen@, seen@.len()) + 1 == before_unseen@);
                assert(pending@.len() + 1 == before_pending@);
                let u = unseen_prefix(seen@, seen@.len());
                let d = self.dependencies@.len();
                let p = pending@.len();
                assert((u + 1) * (d + 1) + (p + 1)
                    == u * (d + 1) + p + d + 2) by(nonlinear_arith);
                assert(u * (d + 1) + p + d < before_metric@);
            }
            let mut edge = 0usize;
            kernel_while! { (edge < self.dependencies.len()) [
                invariant edge <= self.dependencies.len(), edges_indexable(*self),
                    seen.len() == self.nodes.len(),
                    from.0 < self.nodes@.len(), to.0 < self.nodes@.len(),
                    node.0 < self.nodes@.len(), node != to, index == node.0,
                    !(self.nodes@[index as int].phase is Closed), seen@[index as int],
                    !seen@[to.0 as int], reachable(*self, from, node),
                    forall|n: NodeId| n.0 < seen.len() && seen@[n.0 as int]
                        ==> reachable(*self, from, n),
                    explored(*self, seen@, to, from) || pending@.contains(from),
                    forall|a: NodeId, b: NodeId| a != node && explored(*self, seen@, to, a)
                        && active_edge(*self, a, b)
                        ==> explored(*self, seen@, to, b) || pending@.contains(b),
                    forall|j: int| 0 <= j < edge && self.dependencies@[j].node == node
                        && !self.dependencies@[j].rejected
                        && self.dependencies@[j].status == DependencyStatus::Pending
                        && self.dependencies@[j].target.request.is_some()
                        ==> explored(*self, seen@, to, self.dependencies@[j].target.request.unwrap())
                            || pending@.contains(self.dependencies@[j].target.request.unwrap()),
                    unseen_prefix(seen@, seen@.len()) * (self.dependencies@.len() + 1)
                        + pending@.len() + (self.dependencies@.len() - edge) < before_metric@,
                    forall|i: int| 0 <= i < pending@.len() ==> pending@[i].0 < self.nodes@.len()
                        && reachable(*self, from, pending@[i]),
                decreases self.dependencies.len() - edge,
            ] {
                kernel_snapshot! { frontier_scan = pending@ }
                let dependency = self.dependencies[edge];
                if dependency.node == node
                    && !dependency.rejected
                    && dependency.status == DependencyStatus::Pending
                {
                    if let Some(target) = dependency.target.request {
                        kernel_proof! {
                            reveal(active_edge);
                            assert(active_edge(*self, node, target));
                            lemmas::extend(*self, from, node, target);
                        }
                        pending.push(target);
                        kernel_proof! {
                            assert(pending@[pending@.len() - 1] == target);
                            assert(pending@.contains(target));
                        }
                    }
                }
                kernel_proof! {
                    assert forall|n: NodeId| frontier_scan@.contains(n)
                        implies pending@.contains(n) by {
                        let j = choose|j: int| 0 <= j < frontier_scan@.len()
                            && frontier_scan@[j] == n;
                        assert(j < pending@.len());
                        assert(pending@[j] == n);
                    }
                    assert forall|j: int| 0 <= j < edge + 1
                        && self.dependencies@[j].node == node
                        && !self.dependencies@[j].rejected
                        && self.dependencies@[j].status == DependencyStatus::Pending
                        && self.dependencies@[j].target.request.is_some()
                        implies explored(*self, seen@, to, self.dependencies@[j].target.request.unwrap())
                            || pending@.contains(self.dependencies@[j].target.request.unwrap()) by {
                        if j < edge {
                            let target = self.dependencies@[j].target.request.unwrap();
                            if !explored(*self, seen@, to, target) {
                                assert(frontier_scan@.contains(target));
                            }
                        }
                    }
                }
                edge += 1;
            } }
            kernel_proof! {
                assert forall|a: NodeId, b: NodeId| explored(*self, seen@, to, a)
                    && active_edge(*self, a, b)
                    implies explored(*self, seen@, to, b) || pending@.contains(b) by {
                    if a == node {
                        reveal(active_edge);
                        let j = choose|j: int| 0 <= j < self.dependencies@.len()
                            && self.dependencies@[j].node == a
                            && self.dependencies@[j].target.request == Some(b)
                            && !self.dependencies@[j].rejected
                            && self.dependencies@[j].status == DependencyStatus::Pending;
                    }
                }
            }
        } }
    }
    }

    kernel_fn! {
    /// #693 / #771: execute the same per-declaration cycle checks in table order.
    #[allow(clippy::collapsible_if)]
    pub fn dependency_graph_acyclic(&self) -> (result: bool) [
        requires edges_indexable(*self),
        ensures no_active_graph_edges(*self) ==> result,
            result == acyclic(*self),
    ] proof [reveal(edges_indexable); reveal(no_active_graph_edges);
        reveal(active_edge); reveal(acyclic);] {
        let mut index = 0usize;
        kernel_while! { (index < self.dependencies.len()) [
            invariant index <= self.dependencies.len(), edges_indexable(*self),
                forall|j: int| 0 <= j < index
                    && !(self.nodes@[self.dependencies@[j].node.0 as int].phase is Closed)
                    && !self.dependencies@[j].rejected
                    && self.dependencies@[j].status == DependencyStatus::Pending
                    && self.dependencies@[j].target.request.is_some()
                    ==> !reachable(*self, self.dependencies@[j].target.request.unwrap(),
                        self.dependencies@[j].node),
            decreases self.dependencies.len() - index,
        ] {
            let d = &self.dependencies[index];
            if !self.nodes[d.node.0 as usize].phase.is_closed()
                && !d.rejected && d.status == DependencyStatus::Pending
            {
                if let Some(target) = d.target.request {
                    if self.dependency_path(target, d.node) {
                        kernel_proof! {
                            reveal(active_edge);
                            reveal(acyclic);
                            assert(active_edge(*self, d.node, target));
                            assert(!acyclic(*self));
                        }
                        return false;
                    }
                }
            }
            index += 1;
        } }
        kernel_proof! {
            assert forall|a: NodeId, b: NodeId| active_edge(*self, a, b)
                implies !reachable(*self, b, a) by {
                reveal(active_edge);
                let j = choose|j: int| 0 <= j < self.dependencies@.len()
                    && self.dependencies@[j].node == a
                    && self.dependencies@[j].target.request == Some(b)
                    && !self.dependencies@[j].rejected
                    && self.dependencies@[j].status == DependencyStatus::Pending;
            }
            reveal(acyclic);
            assert(acyclic(*self));
        }
        true
    }
    }

    kernel_fn! {
    /// #693 / #771: the real prerequisite checker establishes indexed owners
    /// and prevents a blocked request from holding delivery resources.
    pub(super) fn check_dependencies(&self) -> (result: Result<(), Broken>) [
        requires self.nodes@.len() <= u64::MAX as int,
        ensures result.is_ok() ==> dependencies_checked(*self),
            dependency_retirement::edge_domain(*self)
                ==> result.is_ok() == dependency_retirement::edge_rules(*self),
            progress_unplanned_witness::fixture_edges(*self) ==> result.is_ok(),
            (self.dependencies@.len() == 0
                && forall|j: int| 0 <= j < self.nodes@.len() ==> !self.nodes@[j].credit.withheld)
                ==> result.is_ok(),
    ] proof [reveal(dependencies_checked); reveal(edges_indexable); reveal(no_active_graph_edges); reveal(zero_credit);] {
        let mut node_index = 0usize;
        kernel_while! { (node_index < self.nodes.len()) [
            invariant node_index <= self.nodes.len(), self.nodes@.len() <= u64::MAX as int,
            decreases self.nodes.len() - node_index,
        ] {
            let node = &self.nodes[node_index];
            if node.credit.withheld {
                kernel_proof! {
                    if progress_unplanned_witness::fixture_edges(*self) {
                        assert(node_index == 0);
                        assert(self.dependencies@.len() > 0);
                        assert(self.dependencies@[0].node == NodeId(0));
                    }
                }
                let mut found = false;
                let mut edge = 0usize;
                kernel_while! { (edge < self.dependencies.len()) [
                    invariant edge <= self.dependencies.len(),
                        dependency_retirement::edge_domain(*self) ==> node_index == 0,
                        progress_unplanned_witness::fixture_edges(*self)
                            ==> node_index == 0 && self.dependencies@.len() > 0,
                        !found && progress_unplanned_witness::fixture_edges(*self) ==> edge == 0,
                        !found && dependency_retirement::edge_domain(*self) ==> edge == 0,
                        !found ==> forall|j: int| 0 <= j < edge
                            ==> self.dependencies@[j].node != NodeId(node_index as u64),
                    ensures found || edge == self.dependencies.len(),
                        progress_unplanned_witness::fixture_edges(*self)
                            ==> node_index == 0 && self.dependencies@.len() > 0,
                        !found && progress_unplanned_witness::fixture_edges(*self) ==> edge == 0,
                        !found && dependency_retirement::edge_domain(*self) ==> edge == 0,
                    decreases self.dependencies.len() - edge,
                ] {
                    if self.dependencies[edge].node == NodeId(node_index as u64) {
                        found = true;
                        break;
                    }
                    edge += 1;
                } }
                kernel_proof! {
                    if progress_unplanned_witness::fixture_edges(*self) { assert(found); }
                }
                if !found { return Err(Broken::Dependencies); }
            }
            node_index += 1;
        } }
        if self.dependencies.is_empty() {
            return Ok(());
        }
        let mut index = 0usize;
        kernel_while! { (index < self.dependencies.len()) [
            invariant index <= self.dependencies.len(), self.nodes@.len() <= u64::MAX as int,
                forall|j: int| 0 <= j < index ==> dependency_checked(*self, self.dependencies@[j]),
                dependency_retirement::edge_domain(*self) && index > 0 ==> dependency_retirement::edge_rules(*self),
                forall|a: int, b: int| 0 <= a < b < index
                    ==> !same_declaration(self.dependencies@[a], self.dependencies@[b]),
            decreases self.dependencies.len() - index,
        ] {
            let dependency = &self.dependencies[index];
            let Some(owner) = self.index(dependency.node) else {
                return Err(Broken::Dependencies);
            };
            if dependency.target.number == 0 {
                return Err(Broken::Dependencies);
            }
            if let Some(target) = dependency.target.request {
                match self.index(target) {
                    None => return Err(Broken::Dependencies),
                    Some(target_index) => if self.nodes[target_index].repo != self.nodes[owner].repo {
                        return Err(Broken::Dependencies);
                    },
                }
            }
            let mut previous = 0usize;
            kernel_while! { (previous < index) [
                invariant previous <= index < self.dependencies.len(),
                    *dependency == self.dependencies@[index as int],
                    forall|a: int| 0 <= a < previous
                        ==> !same_declaration(self.dependencies@[a], *dependency),
                decreases index - previous,
            ] {
                let d = &self.dependencies[previous];
                kernel_proof! {
                    if progress_unplanned_witness::fixture_edges(*self) {
                        assert(declarations_unique(*self));
                        assert(!same_declaration(*d, *dependency));
                    }
                }
                if d.node == dependency.node && d.target.kind == dependency.target.kind
                    && d.target.number == dependency.target.number
                { return Err(Broken::Dependencies); }
                previous += 1;
            } }
            if (dependency.rejected || dependency.status != DependencyStatus::Delivered)
                && (self.nodes[owner].planning_started
                    || matches!(self.nodes[owner].question, Some(q) if !matches!(q.cont,
                        Cont::Ingesting | Cont::Auditing | Cont::Unplanned | Cont::Stop(_) | Cont::Nowhere)))
            {
                return Err(Broken::Dependencies);
            }
            if (dependency.rejected || dependency.status != DependencyStatus::Delivered)
                && !matches!(
                    self.nodes[owner].phase,
                    Phase::Ingesting
                        | Phase::Auditing
                        | Phase::Unplanned
                        | Phase::AwaitingRob
                        | Phase::Stopping(_)
                        | Phase::Closed(_)
                )
            {
                return Err(Broken::Dependencies);
            }
            kernel_proof! { reveal(zero_credit); }
            if !self.dependencies_delivered(dependency.node)
                && matches!(
                    self.nodes[owner].phase,
                    Phase::Ingesting | Phase::Auditing | Phase::Unplanned
                )
                && self.nodes[owner].credit.balance() != 0
            {
                return Err(Broken::Dependencies);
            }
            if self.nodes[owner].phase == Phase::Unplanned
                && !self.dependencies_delivered(dependency.node)
            {
                let n = &self.nodes[owner];
                if n.holds
                    || n.pool
                    || n.ticket.is_some()
                    || n.run.is_some()
                    || n.credit.balance() != 0
                {
                    return Err(Broken::Dependencies);
                }
            }
            index += 1;
        } }
        if !self.dependency_graph_acyclic() {
            return Err(Broken::Dependencies);
        }
        Ok(())
    }
    }

    kernel_fn! {
    /// State-table planning admission / #771: withholding preserves ledger amounts.
    pub(super) fn withhold_dependency_credit(&mut self, i: usize) -> (result: ()) [
        requires i < old(self).nodes@.len(),
        ensures final(self).nodes@[i as int] == (Node {
            credit: Credit {
                withheld: old(self).nodes@[i as int].credit.withheld
                    || (!prerequisites_delivered(*old(self), NodeId(i as u64))
                        && !old(self).nodes@[i as int].planning_started
                        && (match old(self).nodes@[i as int].phase {
                            Phase::Ingesting | Phase::Auditing | Phase::Unplanned => true,
                            _ => false,
                        })
                        && old(self).nodes@[i as int].credit.ready),
                ..old(self).nodes@[i as int].credit
            },
            ..old(self).nodes@[i as int]
        }),
            final(self).next_ticket == old(self).next_ticket,
            final(self).next_question == old(self).next_question,
            final(self).gates@ == old(self).gates@,
            final(self).pool_capacity == old(self).pool_capacity,
            final(self).fail_fast == old(self).fail_fast,
            final(self).file_sets@ == old(self).file_sets@,
            final(self).dependencies@ == old(self).dependencies@,
            final(self).nodes@.len() == old(self).nodes@.len(),
            forall|j: int| 0 <= j < old(self).nodes@.len() && j != i
                ==> final(self).nodes@[j] == old(self).nodes@[j],
            final(self).nodes@[i as int].last_cmd == old(self).nodes@[i as int].last_cmd,
            final(self).nodes@[i as int].phase == old(self).nodes@[i as int].phase,
            final(self).nodes@[i as int].pos == old(self).nodes@[i as int].pos,
            final(self).nodes@[i as int].pool == old(self).nodes@[i as int].pool,
            final(self).nodes@[i as int].holds == old(self).nodes@[i as int].holds,
            final(self).nodes@[i as int].generation == old(self).nodes@[i as int].generation,
            final(self).nodes@[i as int].next_seq == old(self).nodes@[i as int].next_seq,
            final(self).nodes@[i as int].credit.start == old(self).nodes@[i as int].credit.start,
            final(self).nodes@[i as int].credit.earned == old(self).nodes@[i as int].credit.earned,
            final(self).nodes@[i as int].credit.added == old(self).nodes@[i as int].credit.added,
            final(self).nodes@[i as int].credit.spent == old(self).nodes@[i as int].credit.spent,
    ] {
        if !self.dependencies_delivered(NodeId(i as u64))
            && !self.nodes[i].planning_started
            && matches!(
                self.nodes[i].phase,
                Phase::Ingesting | Phase::Auditing | Phase::Unplanned
            )
        {
            let n = &mut self.nodes[i];
            if n.credit.ready {
                n.credit.withheld = true;
            }
        }
    }
    }

    kernel_fn! {
    /// Prerequisite removal rows / #693 / #771: stable in-place compaction
    /// preserves every ledger amount and changes only this node's wait flags.
    pub(super) fn remove_dependency(&mut self, i: usize, kind: PrerequisiteKind,
        number: u64) -> (result: ()) [
        requires i < old(self).nodes@.len(),
        ensures final(self).nodes@[i as int] == (Node {
            waiting_prerequisite: final(self).nodes@[i as int].waiting_prerequisite,
            credit: Credit { withheld: final(self).nodes@[i as int].credit.withheld,
                ..old(self).nodes@[i as int].credit },
            ..old(self).nodes@[i as int]
        }),
            final(self).next_ticket == old(self).next_ticket,
            final(self).next_question == old(self).next_question,
            final(self).gates@ == old(self).gates@,
            final(self).pool_capacity == old(self).pool_capacity,
            final(self).fail_fast == old(self).fail_fast,
            final(self).file_sets@ == old(self).file_sets@,
            final(self).nodes@.len() == old(self).nodes@.len(),
            forall|j: int| 0 <= j < old(self).nodes@.len() && j != i
                ==> final(self).nodes@[j] == old(self).nodes@[j],
            final(self).nodes@[i as int].repo == old(self).nodes@[i as int].repo,
            final(self).nodes@[i as int].footprint == old(self).nodes@[i as int].footprint,
            final(self).nodes@[i as int].phase == old(self).nodes@[i as int].phase,
            final(self).nodes@[i as int].pos == old(self).nodes@[i as int].pos,
            final(self).nodes@[i as int].pool == old(self).nodes@[i as int].pool,
            final(self).nodes@[i as int].holds == old(self).nodes@[i as int].holds,
            final(self).nodes@[i as int].last_cmd == old(self).nodes@[i as int].last_cmd,
            final(self).nodes@[i as int].generation == old(self).nodes@[i as int].generation,
            final(self).nodes@[i as int].next_seq == old(self).nodes@[i as int].next_seq,
            final(self).nodes@[i as int].credit.start == old(self).nodes@[i as int].credit.start,
            final(self).nodes@[i as int].credit.earned == old(self).nodes@[i as int].credit.earned,
            final(self).nodes@[i as int].credit.added == old(self).nodes@[i as int].credit.added,
            final(self).nodes@[i as int].credit.spent == old(self).nodes@[i as int].credit.spent,
    ] {
        if self.nodes[i].phase != Phase::Ingesting || self.nodes[i].planning_started {
            return;
        }
        let node = NodeId(i as u64);
        let mut read = 0usize;
        let mut write = 0usize;
        kernel_while! { (read < self.dependencies.len()) [
            invariant write <= read <= self.dependencies.len(),
            decreases self.dependencies.len() - read,
        ] {
            let d = self.dependencies[read];
            if d.node != node || d.target.kind != kind || d.target.number != number {
                self.dependencies[write] = d;
                write += 1;
            }
            read += 1;
        } }
        self.dependencies.truncate(write);
        self.nodes[i].waiting_prerequisite = self.first_undelivered(node).map(|d| d.target.number);
        self.nodes[i].credit.withheld =
            !self.dependencies_delivered(node) && self.nodes[i].credit.ready;
    }
    }

    kernel_fn! {
    /// #693 / #771: find the same first blocking declaration as the public query.
    pub(super) fn first_undelivered(&self, node: NodeId) -> (result: Option<Dependency>) [
        ensures dependency_retirement::edge_domain(*self) && node == NodeId(0) ==> result ==
                if self.dependencies@[0].status == DependencyStatus::Delivered { None } else { Some(self.dependencies@[0]) },
            match result {
            Some(d) => d.node == node && (d.rejected || d.status != DependencyStatus::Delivered)
                && exists|j: int| 0 <= j < self.dependencies@.len() && self.dependencies@[j] == d,
            None => prerequisites_delivered(*self, node),
        },
    ] proof [reveal(prerequisites_delivered); reveal(delivered);] {
        let mut index = 0usize;
        kernel_while! { (index < self.dependencies.len()) [
            invariant index <= self.dependencies.len(),
                forall|j: int| 0 <= j < index && self.dependencies@[j].node == node
                    ==> !self.dependencies@[j].rejected
                        && self.dependencies@[j].status == DependencyStatus::Delivered,
            decreases self.dependencies.len() - index,
        ] {
            let d = self.dependencies[index];
            if d.node == node && (d.rejected || d.status != DependencyStatus::Delivered) {
                return Some(d);
            }
            index += 1;
        } }
        None
    }
    }

    kernel_fn! {
    /// Prerequisite fact rows / #693 / #771: declarations and terminal facts
    /// preserve every numeric ledger and emit at most one note.
    #[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
    pub(super) fn dependency_fact(&mut self, i: usize, target: DependencyTarget,
        status: DependencyStatus, out: &mut Outbox) -> (result: ()) [
        requires i < old(self).nodes@.len(), i <= u64::MAX as int,
            edges_indexable(*old(self)), old(out).len < OUTBOX,
        ensures dependency_frame(*old(self), *final(self), i as int),
            dependency_retirement::edge_domain(*old(self)) && i == 0
                && target == old(self).dependencies@[0].target ==>
                    dependency_retirement::fact_result(*old(self), *final(self), status, *old(out), *final(out)),
            final(out).len <= old(out).len + 1,
            (status == DependencyStatus::Delivered && delivery_input(*old(self), i as int, target)
                ==> delivery_changed(*old(self), *final(self), i as int, target)
                    && progress::progress_measure(*final(self)) + 1 == progress::progress_measure(*old(self))
                    && edges_indexable(*final(self))),
    ] {
        kernel_snapshot! { before_delivery = *self }
        kernel_snapshot! { before_out = *out }
        let Some((existing, dependency)) = self.prepare_dependency(i, target, status) else {
            kernel_proof! {
                reveal(dependency_frame);
                if dependency_retirement::edge_domain(*self) && i == 0 && target == self.dependencies@[0].target {
                    assert(dependency_retirement::fact_result(before_delivery@, *self, status, before_out@, *out));
                }
            }
            return;
        };
        self.record_dependency(i, existing, dependency, out);
        kernel_proof! {
            reveal(dependency_frame);
            if status == DependencyStatus::Delivered && delivery_input(before_delivery@, i as int, target) {
                reveal(delivery_input);
                let k = existing.unwrap() as int;
                progress::delivery_rank_delta(before_delivery@, *self, i as int, k);
                reveal(delivery_changed);
                assert(before_delivery@.dependencies@[k].target == target);
            }
        }
    }
    }

    kernel_fn! {
    /// #693 / #771: validate the unchanged declaration before mutating its table.
    /// Read-only guards and cycle queries do not change any allocation counter.
    fn prepare_dependency(&self, i: usize, target: DependencyTarget,
        status: DependencyStatus) -> (result: Option<(Option<usize>, Dependency)>) [
        requires i < self.nodes@.len(), i <= u64::MAX as int, edges_indexable(*self),
        ensures dependency_retirement::edge_domain(*self) && i == 0 && target == self.dependencies@[0].target
                ==> dependency_retirement::prepared(*self, status, result),
            match result {
            Some((Some(index), _)) => index < self.dependencies@.len(),
            _ => true,
        },
            (status == DependencyStatus::Delivered && delivery_input(*self, i as int, target)
                ==> match result {
                    Some((Some(k), d)) => k < self.dependencies@.len()
                        && self.dependencies@[k as int].node == NodeId(i as u64)
                        && self.dependencies@[k as int].target == target
                        && self.dependencies@[k as int].status == DependencyStatus::Pending
                        && d == (Dependency { status: DependencyStatus::Delivered, rejected: false,
                            ..self.dependencies@[k as int] }),
                    _ => false,
                }),
    ] proof [reveal(delivery_input);] {
        if target.number == 0 {
            return None;
        }
        if let Some(node) = target.request {
            let j = self.index(node)?;
            if self.nodes[i].repo != self.nodes[j].repo {
                return None;
            }
        }
        let node = NodeId(i as u64);
        let existing = self.find_dependency(node, target.kind, target.number);
        kernel_proof! {
            if status == DependencyStatus::Delivered && delivery_input(*self, i as int, target) {
                assert(existing is Some);
            }
        }
        if let Some(index) = existing {
            let previous = self.dependencies[index];
            let owner = &self.nodes[i];
            // #839: preserve the existing dependency frame for every valid world,
            // including restored pre-planning questions and teardown phases.
            let revocable = !owner.planning_started
                && matches!(owner.phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned
                    | Phase::AwaitingRob | Phase::Stopping(_) | Phase::Closed(_))
                && !matches!(owner.question, Some(q) if !matches!(q.cont,
                    Cont::Ingesting | Cont::Auditing | Cont::Unplanned | Cont::Stop(_) | Cont::Nowhere))
                && (owner.phase != Phase::Unplanned
                    || (!owner.holds && !owner.pool && owner.ticket.is_none() && owner.run.is_none()));
            if !update_allowed(previous.status, previous.rejected, status, revocable) {
                return None;
            }
            if previous.status == DependencyStatus::Pending
                && status == DependencyStatus::Pending && target.request == previous.target.request {
                return None;
            }
        } else if self.nodes[i].planning_started
            || !matches!(
                self.nodes[i].phase,
                Phase::Ingesting | Phase::Auditing | Phase::Unplanned
            )
        {
            return None;
        }
        let rejected = if status == DependencyStatus::Pending {
            match target.request {
                Some(target) => self.dependency_path(target, node),
                None => false,
            }
        } else { false };
        let dependency = Dependency {
            node,
            target,
            status,
            rejected,
        };
        Some((existing, dependency))
    }
    }

    kernel_fn! {
    /// #693 / #771: store the validated declaration, then reconcile wait flags.
    /// This separates the unchanged writes from cycle-query validation.
    // The pinned Verus frontend needs an explicit match to retain the exact wait number.
    #[allow(clippy::manual_map)]
    #[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
    fn record_dependency(&mut self, i: usize, existing: Option<usize>,
        dependency: Dependency, out: &mut Outbox) -> (result: ()) [
        requires i < old(self).nodes@.len(), old(out).len < OUTBOX,
            match existing { None => true, Some(index) => index < old(self).dependencies@.len() },
        ensures dependency_frame(*old(self), *final(self), i as int),
            final(self).dependencies@ == match existing {
                Some(index) => old(self).dependencies@.update(index as int, dependency),
                None => old(self).dependencies@.push(dependency),
            },
            final(out).len <= old(out).len + 1,
            !dependency.rejected && dependency.status != DependencyStatus::Abandoned ==> *final(out) == *old(out),
            !dependency.rejected && dependency.status == DependencyStatus::Abandoned ==>
                final(out).len == old(out).len + 1 && final(out).items@[old(out).len as int]
                    == Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Note(
                        NoteKind::DependencyAbandoned { number: dependency.target.number }) }),
            dependency_retirement::edge_domain(*old(self)) && i == 0 && existing == Some(0usize)
                && dependency.node == NodeId(0) && dependency.target == old(self).dependencies@[0].target
                && !dependency.rejected ==> final(self).nodes@[0] == (Node {
                    waiting_prerequisite: if dependency.status == DependencyStatus::Delivered { None } else { Some(2761u64) },
                    credit: Credit { withheld: old(self).nodes@[0].credit.withheld
                        || (dependency.status != DependencyStatus::Delivered && !old(self).nodes@[0].planning_started
                            && (match old(self).nodes@[0].phase { Phase::Ingesting | Phase::Auditing | Phase::Unplanned => true, _ => false })
                            && old(self).nodes@[0].credit.ready), ..old(self).nodes@[0].credit },
                    ..old(self).nodes@[0]
                }),
    ] proof [reveal(dependency_frame); reveal(prerequisites_delivered); reveal(delivered);] {
        kernel_snapshot! { before_record = *self }
        let node = NodeId(i as u64);
        match existing {
            Some(index) => self.dependencies[index] = dependency,
            None => self.dependencies.push(dependency),
        }
        if dependency.rejected {
            notice(
                out,
                i,
                Notice::Note(NoteKind::DependencyCycle {
                    number: dependency.target.number,
                }),
            );
        } else if dependency.status == DependencyStatus::Abandoned {
            notice(
                out,
                i,
                Notice::Note(NoteKind::DependencyAbandoned {
                    number: dependency.target.number,
                }),
            );
        }
        kernel_proof! {
            if dependency_retirement::edge_domain(before_record@) && i == 0 && existing == Some(0usize)
                && dependency.node == NodeId(0) && dependency.target == before_record@.dependencies@[0].target
                && !dependency.rejected {
                assert(dependency_retirement::edge_domain(*self));
                assert(self.dependencies@[0] == dependency);
            }
        }
        let waiting = self.first_undelivered(node);
        self.nodes[i].waiting_prerequisite = match waiting {
            Some(d) => Some(d.target.number), None => None,
        };
        kernel_proof! {
            if dependency_retirement::edge_domain(before_record@) && i == 0 && existing == Some(0usize)
                && dependency.node == NodeId(0) && dependency.target == before_record@.dependencies@[0].target
                && !dependency.rejected {
                assert(self.nodes@[0].waiting_prerequisite == if dependency.status == DependencyStatus::Delivered { None } else { Some(2761u64) });
            }
        }
        self.withhold_dependency_credit(i);
    }
    }

    kernel_fn! {
    /// #693 / #771: declaration identity selects the first matching table row.
    fn find_dependency(&self, node: NodeId, kind: PrerequisiteKind,
        number: u64) -> (result: Option<usize>) [
        ensures match result {
            None => forall|i: int| 0 <= i < self.dependencies@.len() ==>
                self.dependencies@[i].node != node || self.dependencies@[i].target.kind != kind
                    || self.dependencies@[i].target.number != number,
            Some(i) => i < self.dependencies@.len() && self.dependencies@[i as int].node == node
                && self.dependencies@[i as int].target.kind == kind
                && self.dependencies@[i as int].target.number == number,
        },
    ] {
        let mut index = 0usize;
        kernel_while! { (index < self.dependencies.len()) [
            invariant index <= self.dependencies.len(),
                forall|j: int| 0 <= j < index ==> self.dependencies@[j].node != node
                    || self.dependencies@[j].target.kind != kind
                    || self.dependencies@[j].target.number != number,
            decreases self.dependencies.len() - index,
        ] {
            let d = self.dependencies[index];
            if d.node == node && d.target.kind == kind && d.target.number == number {
                return Some(index);
            }
            index += 1;
        } }
        None
    }
    }
}

#[cfg(verus_keep_ghost)]
verus! {
/// Prerequisite rows / #806: the original empty gate table has no delivery obstruction.
pub(super) proof fn empty_delivered(w: World, node: NodeId)
    requires w.dependencies@.len() == 0,
    ensures delivered(w.dependencies@, node),
{
    reveal(delivered);
}
}
