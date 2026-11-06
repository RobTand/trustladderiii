//! #693 / #806 W7: exact external prerequisite updates and stable removal.
use super::port_deps_specs::*;
use super::*;

verus! {
/// #806 / #839: external-row preparation follows revocable delivery and abandoned recovery.
/// The revocation predicate is the frozen owner definition, not a second policy.
#[verifier::opaque]
pub(super) open spec fn prepared(w: World, i: int, target: DependencyTarget,
    status: DependencyStatus) -> Option<(Option<usize>, Dependency)> {
    let found = exists|k: usize| k < w.dependencies@.len() && key(w.dependencies@[k as int], i, target);
    let existing = if found {
        Some(choose|k: usize| k < w.dependencies@.len() && key(w.dependencies@[k as int], i, target))
    } else { None };
    if target.number == 0 || (match existing {
        Some(k) => w.dependencies@[k as int].rejected
            || !(w.dependencies@[k as int].status == DependencyStatus::Pending
                || (w.dependencies@[k as int].status == DependencyStatus::Abandoned && status == DependencyStatus::Delivered)
                || (dependency_retirement::revocable(w.nodes@[i]) && status == DependencyStatus::Pending))
            || (w.dependencies@[k as int].status == DependencyStatus::Pending
                && status == DependencyStatus::Pending && target.request == w.dependencies@[k as int].target.request),
        None => w.nodes@[i].planning_started || !(match w.nodes@[i].phase { Phase::Ingesting | Phase::Auditing | Phase::Unplanned => true, _ => false }),
    }) { None } else { Some((existing, Dependency { node: NodeId(i as u64), target, status, rejected: false })) }
}

/// #806: the real prepare/record result changes only this selected declaration.
#[verifier::opaque]
pub(super) open spec fn external_post(w: World, v: World, i: int,
    target: DependencyTarget, status: DependencyStatus, out: Outbox, next: Outbox) -> bool {
    dependencies::dependency_frame(w, v, i) && match prepared(w, i, target, status) {
        None => unchanged(w, v) && next == out,
        Some((existing, d)) => v.dependencies@ == match existing {
            None => w.dependencies@.push(d), Some(k) => w.dependencies@.update(k as int, d),
        } && recorded_output(out, next, i as usize, d)
            && v.nodes@[i].waiting_prerequisite.is_none() == dependencies::prerequisites_delivered(v, NodeId(i as u64))
            && v.nodes@[i].credit.withheld == (w.nodes@[i].credit.withheld
                || (!dependencies::prerequisites_delivered(v, NodeId(i as u64))
                    && !w.nodes@[i].planning_started && w.nodes@[i].credit.ready
                    && (match w.nodes@[i].phase { Phase::Ingesting | Phase::Auditing | Phase::Unplanned => true, _ => false }))),
    }
}

/// #806 declaration identity: the real search agrees with the unique specification key.
pub(super) proof fn existing_key(w: World, i: int, target: DependencyTarget, existing: Option<usize>)
    requires dependencies::declarations_unique(w),
        match existing {
            None => forall|k: int| 0 <= k < w.dependencies@.len() ==> !key(w.dependencies@[k], i, target),
            Some(k) => k < w.dependencies@.len() && key(w.dependencies@[k as int], i, target),
        },
    ensures existing == if exists|k: usize| k < w.dependencies@.len() && key(w.dependencies@[k as int], i, target) {
        Some(choose|k: usize| k < w.dependencies@.len() && key(w.dependencies@[k as int], i, target))
    } else { None },
{
    reveal(dependencies::declarations_unique); reveal(key);
    if let Some(k) = existing {
        assert(exists|j: usize| j < w.dependencies@.len() && key(w.dependencies@[j as int], i, target)) by {
            assert(key(w.dependencies@[k as int], i, target));
        }
        let j = choose|j: usize| j < w.dependencies@.len() && key(w.dependencies@[j as int], i, target);
        if j < k { assert(!dependencies::same_declaration(w.dependencies@[j as int], w.dependencies@[k as int])); }
        if k < j { assert(!dependencies::same_declaration(w.dependencies@[k as int], w.dependencies@[j as int])); }
    }
}

/// #806 removal: the stable retained prefix of the real compaction loop.
#[verifier::opaque]
pub(super) open spec fn kept(edges: Seq<Dependency>, node: NodeId,
    kind: PrerequisiteKind, number: u64, end: nat) -> Seq<Dependency>
    recommends end <= edges.len(),
    decreases end,
{
    if end == 0 { Seq::empty() } else {
        let previous = kept(edges, node, kind, number, (end - 1) as nat);
        let d = edges[end as int - 1];
        if d.node == node && d.target.kind == kind && d.target.number == number {
            previous
        } else { previous.push(d) }
    }
}

/// #806 stable removal: the empty retained prefix is empty for every real table.
pub(super) proof fn kept_empty(edges: Seq<Dependency>, node: NodeId, kind: PrerequisiteKind, number: u64)
    ensures kept(edges, node, kind, number, 0) == Seq::<Dependency>::empty(),
{ reveal(kept); }

/// #806 stable removal: unfold one table row without exposing recursive compaction to callers.
pub(super) proof fn kept_step(edges: Seq<Dependency>, node: NodeId, kind: PrerequisiteKind,
    number: u64, read: nat)
    requires read < edges.len(),
    ensures kept(edges, node, kind, number, read + 1) == if edges[read as int].node == node
        && edges[read as int].target.kind == kind && edges[read as int].target.number == number {
        kept(edges, node, kind, number, read)
    } else { kept(edges, node, kind, number, read).push(edges[read as int]) },
{
    reveal(kept);
}

/// #806 stable removal: one real in-place copy preserves the retained prefix and unread suffix.
pub(super) proof fn compact_copy(edges: Seq<Dependency>, original: Seq<Dependency>, node: NodeId,
    kind: PrerequisiteKind, number: u64, read: int, write: int)
    requires 0 <= write <= read < edges.len(), edges.len() == original.len(),
        edges.subrange(0, write) == kept(original, node, kind, number, read as nat),
        forall|k: int| read <= k < edges.len() ==> edges[k] == original[k],
    ensures (edges[read].node != node || edges[read].target.kind != kind || edges[read].target.number != number)
        ==> edges.update(write, edges[read]).subrange(0, write + 1)
            == kept(original, node, kind, number, (read + 1) as nat),
        (edges[read].node == node && edges[read].target.kind == kind && edges[read].target.number == number)
        ==> edges.subrange(0, write) == kept(original, node, kind, number, (read + 1) as nat),
{
    kept_step(original, node, kind, number, read as nat);
    let d = edges[read];
    if d.node != node || d.target.kind != kind || d.target.number != number {
        assert(edges.update(write, d).subrange(0, write + 1) =~= edges.subrange(0, write).push(d));
    }
}

/// #806 stable removal: hide the retained prefix and unread suffix from the exec loop.
#[verifier::opaque]
pub(super) open spec fn compacted(edges: Seq<Dependency>, original: Seq<Dependency>, node: NodeId,
    kind: PrerequisiteKind, number: u64, read: int, write: int) -> bool {
    0 <= write <= read <= edges.len() && edges.len() == original.len()
        && edges.subrange(0, write) == kept(original, node, kind, number, read as nat)
        && forall|k: int| read <= k < edges.len() ==> edges[k] == original[k]
}

/// #806 stable removal: the real loop starts with an empty retained prefix.
pub(super) proof fn compact_start(edges: Seq<Dependency>, node: NodeId,
    kind: PrerequisiteKind, number: u64)
    ensures compacted(edges, edges, node, kind, number, 0, 0),
{
    reveal(compacted); kept_empty(edges, node, kind, number);
    assert(edges.subrange(0, 0) =~= Seq::<Dependency>::empty());
}

/// #806 stable removal: one real copy or skipped row advances the opaque loop frame.
pub(super) proof fn compact_step(edges: Seq<Dependency>, original: Seq<Dependency>, node: NodeId,
    kind: PrerequisiteKind, number: u64, read: int, write: int)
    requires compacted(edges, original, node, kind, number, read, write), read < edges.len(),
    ensures if edges[read].node != node || edges[read].target.kind != kind || edges[read].target.number != number {
        compacted(edges.update(write, edges[read]), original, node, kind, number, read + 1, write + 1)
    } else { compacted(edges, original, node, kind, number, read + 1, write) },
{
    reveal(compacted);
    compact_copy(edges, original, node, kind, number, read, write);
    assert forall|k: int| read < k < edges.len()
        implies edges.update(write, edges[read])[k] == original[k] by {}
}

/// #806 stable removal: loop exhaustion exposes the exact prefix before real truncate.
pub(super) proof fn compact_finish(edges: Seq<Dependency>, original: Seq<Dependency>, node: NodeId,
    kind: PrerequisiteKind, number: u64, read: int, write: int)
    requires compacted(edges, original, node, kind, number, read, write), read == edges.len(),
    ensures edges.subrange(0, write) == kept(original, node, kind, number, original.len()),
{
    reveal(compacted);
}

/// #806 removal: exact stable table and wait/credit reconciliation, with no count bound.
#[verifier::opaque]
pub(super) open spec fn removed(w: World, v: World, i: int,
    kind: PrerequisiteKind, number: u64) -> bool {
    dependencies::dependency_frame(w, v, i)
        && if w.nodes@[i].phase != Phase::Ingesting || w.nodes@[i].planning_started {
            unchanged(w, v)
        } else {
            v.dependencies@ == kept(w.dependencies@, NodeId(i as u64), kind, number, w.dependencies@.len())
                && v.nodes@[i].waiting_prerequisite.is_none() == dependencies::prerequisites_delivered(v, NodeId(i as u64))
                && v.nodes@[i].credit.withheld == (!dependencies::prerequisites_delivered(v, NodeId(i as u64))
                    && w.nodes@[i].credit.ready)
        }
}

/// #806 removal: a refused removal preserves the complete world and local frame.
pub(super) proof fn removal_stutter(w: World, i: int, kind: PrerequisiteKind, number: u64)
    requires 0 <= i < w.nodes@.len(),
        w.nodes@[i].phase != Phase::Ingesting || w.nodes@[i].planning_started,
    ensures removed(w, w, i, kind, number), dependencies::dependency_frame(w, w, i),
{
    reveal(removed); reveal(unchanged); reveal(dependencies::dependency_frame);
}

/// #806 stable removal: fold the already-proved table, wait and node frames locally.
pub(super) proof fn removal_result(w: World, v: World, i: int,
    kind: PrerequisiteKind, number: u64)
    requires 0 <= i < w.nodes@.len(), dependencies::dependency_frame(w, v, i),
        w.nodes@[i].phase == Phase::Ingesting, !w.nodes@[i].planning_started,
        v.dependencies@ == kept(w.dependencies@, NodeId(i as u64), kind, number, w.dependencies@.len()),
        v.nodes@[i].waiting_prerequisite.is_none() == dependencies::delivered(v.dependencies@, NodeId(i as u64)),
        v.nodes@[i].credit.withheld == (!dependencies::delivered(v.dependencies@, NodeId(i as u64))
            && w.nodes@[i].credit.ready),
    ensures removed(w, v, i, kind, number),
{ reveal(removed); dependencies::prerequisite_definition(v, NodeId(i as u64)); }
}
