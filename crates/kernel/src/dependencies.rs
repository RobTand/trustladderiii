use super::*;

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
/// State-table planning admission / #693 / #771: every prerequisite is delivered.
pub closed spec fn delivered(dependencies: Seq<Dependency>, node: NodeId) -> bool {
    forall|i: int| 0 <= i < dependencies.len() && dependencies[i].node == node
        ==> !dependencies[i].rejected && dependencies[i].status == DependencyStatus::Delivered
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

/// #771: expose the existing-index invariant to the enclosing kernel proofs.
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
                    && world.nodes@[dependency.node.0 as int].run.is_none())
}

/// #693 / #771: executable prerequisite checks imply this progress projection.
pub(super) open spec fn dependencies_checked(world: World) -> bool {
    edges_indexable(world) && declarations_unique(world)
        && forall|j: int| 0 <= j < world.dependencies@.len()
            ==> dependency_checked(world, world.dependencies@[j])
}

/// #693 / #771: duplicate declaration keys are rejected by check_dependencies.
pub(super) open spec fn same_declaration(a: Dependency, b: Dependency) -> bool {
    a.node == b.node && a.target.kind == b.target.kind && a.target.number == b.target.number
}
pub(super) open spec fn declarations_unique(w: World) -> bool {
    forall|a: int, b: int| 0 <= a < b < w.dependencies@.len()
        ==> !same_declaration(w.dependencies@[a], w.dependencies@[b])
}

/// #771: the production key lookup selects the unique pending witness row.
proof fn pending_lookup(w: World, i: int, target: DependencyTarget, existing: Option<usize>)
    requires dependencies_checked(w), dependency_progress_input(w, i, target, DependencyStatus::Delivered),
        match existing {
            Some(index) => index < w.dependencies@.len()
                && same_declaration(w.dependencies@[index as int], Dependency {
                    node: NodeId(i as u64), target, status: DependencyStatus::Delivered, rejected: false }),
            None => forall|j: int| 0 <= j < w.dependencies@.len() ==>
                !same_declaration(w.dependencies@[j], Dependency {
                    node: NodeId(i as u64), target, status: DependencyStatus::Delivered, rejected: false }),
        },
    ensures existing is Some,
        w.dependencies@[existing.unwrap() as int].target == target,
        w.dependencies@[existing.unwrap() as int].node == NodeId(i as u64),
        !w.dependencies@[existing.unwrap() as int].rejected,
        w.dependencies@[existing.unwrap() as int].status == DependencyStatus::Pending,
        target.number > 0,
        match target.request { None => true, Some(node) => node.0 < w.nodes@.len()
            && w.nodes@[node.0 as int].repo == w.nodes@[i].repo },
{
    let k = choose|k: int| 0 <= k < w.dependencies@.len()
        && w.dependencies@[k].node == NodeId(i as u64) && w.dependencies@[k].target == target
        && !w.dependencies@[k].rejected && w.dependencies@[k].status == DependencyStatus::Pending;
    assert(existing is Some);
    let index = existing.unwrap() as int;
    if index < k { assert(!same_declaration(w.dependencies@[index], w.dependencies@[k])); }
    if k < index { assert(!same_declaration(w.dependencies@[k], w.dependencies@[index])); }
    assert(index == k);
}

/// #693 / #771: an exact delivered-row replacement consumes one debt token.
proof fn delivered_record_progress(before: World, after: World, index: int)
    requires 0 <= index < before.dependencies@.len(), edges_indexable(before),
        before.dependencies@[index].status == DependencyStatus::Pending,
        !(before.nodes@[before.dependencies@[index].node.0 as int].phase is Closed),
        after.dependencies@ == before.dependencies@.update(index, Dependency {
            status: DependencyStatus::Delivered, rejected: false, ..before.dependencies@[index] }),
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() ==> after.nodes@[j].phase == before.nodes@[j].phase,
    ensures edges_indexable(after),
        (forall|j: int| 0 <= j < before.dependencies@.len()
            ==> dependency_weight(after, j) <= dependency_weight(before, j)),
        dependency_weight(after, index) < dependency_weight(before, index),
{
    reveal(edges_indexable);
    assert forall|j: int| 0 <= j < before.dependencies@.len() implies
        dependency_weight(after, j) <= dependency_weight(before, j) by {
        if j != index { assert(after.dependencies@[j] == before.dependencies@[j]); }
    }
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
    ] proof [reveal(edges_indexable);] {
        if self.nodes.len() <= 64 {
            // Most worlds fit in a bitset. Repeated edge relaxation reaches
            // the same nodes as the unbounded traversal below, without allocation.
            let mut seen = 1u64 << from.0;
            kernel_loop! { [
                invariant self.nodes.len() <= 64, to.0 < self.nodes@.len(),
                    edges_indexable(*self),
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
                            kernel_proof! { assert((seen | mask) >= seen) by(bit_vector); }
                            seen |= mask;
                        }
                    }
                    index += 1;
                } }
                if seen == before {
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
                forall|i: int| 0 <= i < pending@.len() ==> pending@[i].0 < self.nodes@.len(),
            decreases unseen_prefix(seen@, seen@.len()) * (self.dependencies@.len() + 1)
                + pending@.len(),
        ] {
            kernel_snapshot! { before_metric = unseen_prefix(seen@, seen@.len())
                * (self.dependencies@.len() + 1) + pending@.len() }
            kernel_snapshot! { before_unseen = unseen_prefix(seen@, seen@.len()) }
            kernel_snapshot! { before_pending = pending@.len() }
            let Some(node) = pending.pop() else { return false; };
            if node == to {
                return true;
            }
            let Some(index) = self.index(node) else {
                continue;
            };
            if seen[index] || self.nodes[index].phase.is_closed() {
                continue;
            }
            kernel_proof! { visit_reduces_unseen(seen@, index as int, seen@.len()); }
            seen[index] = true;
            kernel_proof! {
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
                    unseen_prefix(seen@, seen@.len()) * (self.dependencies@.len() + 1)
                        + pending@.len() + (self.dependencies@.len() - edge) < before_metric@,
                    forall|i: int| 0 <= i < pending@.len() ==> pending@[i].0 < self.nodes@.len(),
                decreases self.dependencies.len() - edge,
            ] {
                let dependency = self.dependencies[edge];
                if dependency.node == node
                    && !dependency.rejected
                    && dependency.status == DependencyStatus::Pending
                {
                    if let Some(target) = dependency.target.request {
                        pending.push(target);
                    }
                }
                edge += 1;
            } }
        } }
    }
    }

    kernel_fn! {
    /// #693 / #771: execute the same per-declaration cycle checks in table order.
    #[allow(clippy::collapsible_if)]
    pub fn dependency_graph_acyclic(&self) -> (result: bool) [
        requires edges_indexable(*self),
    ] proof [reveal(edges_indexable);] {
        let mut index = 0usize;
        kernel_while! { (index < self.dependencies.len()) [
            invariant index <= self.dependencies.len(), edges_indexable(*self),
            decreases self.dependencies.len() - index,
        ] {
            let d = &self.dependencies[index];
            if !self.nodes[d.node.0 as usize].phase.is_closed()
                && !d.rejected && d.status == DependencyStatus::Pending
            {
                if let Some(target) = d.target.request {
                    if self.dependency_path(target, d.node) { return false; }
                }
            }
            index += 1;
        } }
        true
    }
    }

    kernel_fn! {
    /// #693 / #771: the real prerequisite checker establishes indexed owners
    /// and prevents a blocked request from holding delivery resources.
    pub(super) fn check_dependencies(&self) -> (result: Result<(), Broken>) [
        requires self.nodes@.len() <= u64::MAX as int,
        ensures result.is_ok() ==> dependencies_checked(*self),
    ] proof [reveal(dependencies_checked); reveal(edges_indexable);] {
        let mut node_index = 0usize;
        kernel_while! { (node_index < self.nodes.len()) [
            invariant node_index <= self.nodes.len(), self.nodes@.len() <= u64::MAX as int,
            decreases self.nodes.len() - node_index,
        ] {
            let node = &self.nodes[node_index];
            if node.credit.withheld {
                let mut found = false;
                let mut edge = 0usize;
                kernel_while! { (edge < self.dependencies.len()) [
                    invariant edge <= self.dependencies.len(),
                    decreases self.dependencies.len() - edge,
                ] {
                    if self.dependencies[edge].node == NodeId(node_index as u64) {
                        found = true;
                        break;
                    }
                    edge += 1;
                } }
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
        ensures final(self).nodes@ == old(self).nodes@.update(i as int, Node {
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
        ensures final(self).nodes@ == old(self).nodes@.update(i as int, Node {
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
        ensures match result {
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
        ensures (dependencies_checked(*old(self)) && dependency_progress_input(*old(self), i as int, target, status)
            ==> edges_indexable(*final(self))
                && final(self).dependencies@.len() == old(self).dependencies@.len()
                && (forall|j: int| 0 <= j < old(self).dependencies@.len() ==>
                    dependency_weight(*final(self), j) <= dependency_weight(*old(self), j))
                && (exists|j: int| 0 <= j < old(self).dependencies@.len()
                    && dependency_weight(*final(self), j) < dependency_weight(*old(self), j))),
            final(self).nodes@ == old(self).nodes@.update(i as int, Node {
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
            final(out).len <= old(out).len + 1,
    ] {
        kernel_snapshot! { before_dependency = *self }
        kernel_proof! {
            if dependencies_checked(*self) && dependency_progress_input(*self, i as int, target, status) {
                let k = choose|k: int| 0 <= k < self.dependencies@.len()
                    && self.dependencies@[k].node == NodeId(i as u64) && self.dependencies@[k].target == target
                    && !self.dependencies@[k].rejected && self.dependencies@[k].status == DependencyStatus::Pending;
                assert(target.number > 0);
                assert(match target.request { None => true, Some(node) => node.0 < self.nodes@.len()
                    && self.nodes@[node.0 as int].repo == self.nodes@[i as int].repo });
            }
        }
        if target.number == 0 {
            return;
        }
        if let Some(node) = target.request {
            match self.index(node) {
                None => return,
                Some(j) => if self.nodes[i].repo != self.nodes[j].repo { return; },
            }
        }
        let node = NodeId(i as u64);
        let existing = self.find_dependency(node, target.kind, target.number);
        kernel_proof! {
            if dependencies_checked(*self) && dependency_progress_input(*self, i as int, target, status) {
                pending_lookup(*self, i as int, target, existing);
            }
        }
        if let Some(index) = existing {
            let previous = self.dependencies[index];
            if previous.rejected || previous.status != DependencyStatus::Pending {
                return;
            }
            if status == DependencyStatus::Pending && target.request == previous.target.request {
                return;
            }
        } else if self.nodes[i].planning_started
            || !matches!(
                self.nodes[i].phase,
                Phase::Ingesting | Phase::Auditing | Phase::Unplanned
            )
        {
            return;
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
        self.record_dependency(i, existing, dependency, out);
        kernel_proof! {
            if dependency_progress_input(before_dependency@, i as int, target, status) {
                delivered_record_progress(before_dependency@, *self, existing.unwrap() as int);
            }
        }
    }
    }

    kernel_fn! {
    /// #693 / #771: store the validated declaration, then reconcile wait flags.
    /// This separates the unchanged writes from cycle-query validation.
    #[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
    fn record_dependency(&mut self, i: usize, existing: Option<usize>,
        dependency: Dependency, out: &mut Outbox) -> (result: ()) [
        requires i < old(self).nodes@.len(), old(out).len < OUTBOX,
            match existing { None => true, Some(index) => index < old(self).dependencies@.len() },
        ensures final(self).dependencies@ == match existing {
                Some(index) => old(self).dependencies@.update(index as int, dependency),
                None => old(self).dependencies@.push(dependency),
            },
            final(self).nodes@ == old(self).nodes@.update(i as int, Node {
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
            final(out).len <= old(out).len + 1,
    ] {
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
        self.nodes[i].waiting_prerequisite = self.first_undelivered(node).map(|d| d.target.number);
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
