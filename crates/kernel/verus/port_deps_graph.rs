//! #693 / #806 W7: graph cuts for the real prerequisite bitset traversal.
use super::*;
use vstd::iset::ISet;

verus! {
/// State-table wait graph: terminal/rejected declarations and closed owners
/// contribute no active edge. The owner/target index check remains production's.
pub(super) open spec fn active(w: World, d: Dependency) -> bool {
    !d.rejected && d.status == DependencyStatus::Pending && d.target.request.is_some()
        && !(w.nodes@[d.node.0 as int].phase is Closed)
}

/// #806: a graph cut contains the target of every active outgoing declaration.
pub(super) open spec fn closed(w: World, cut: ISet<NodeId>) -> bool {
    forall|k: int| 0 <= k < w.dependencies@.len() && active(w, w.dependencies@[k])
        && cut.contains(w.dependencies@[k].node)
        ==> cut.contains(w.dependencies@[k].target.request.unwrap())
}

/// #806: reachability is membership in every closed set containing the source.
/// This is the least reflexive, edge-closed relation on the actual table.
#[verifier::opaque]
pub(super) open spec fn reaches(w: World, from: NodeId, to: NodeId) -> bool {
    forall|cut: ISet<NodeId>| closed(w, cut) && cut.contains(from) ==> cut.contains(to)
}

/// #806: each active edge has no return path through the same real table.
pub(super) open spec fn acyclic(w: World) -> bool {
    forall|k: int| 0 <= k < w.dependencies@.len() && active(w, w.dependencies@[k])
        ==> !reaches(w, w.dependencies@[k].target.request.unwrap(), w.dependencies@[k].node)
}

/// #806: exact set represented by production's u64 visited mask.
#[verifier::opaque]
pub(super) open spec fn marked(mask: u64) -> ISet<NodeId> {
    ISet::new(|n: NodeId| n.0 < 64 && mask & (1u64 << n.0) != 0)
}

/// #806: every marked root has been reached through actual active declarations.
pub(super) open spec fn sound_mask(w: World, from: NodeId, mask: u64) -> bool {
    marked(mask).contains(from)
        && forall|n: NodeId| marked(mask).contains(n)
            ==> n.0 < w.nodes@.len() && reaches(w, from, n)
}

/// #806 bitset representation: a singleton mask marks exactly its root.
pub(super) proof fn singleton_bit(from: u64, other: u64)
    requires from < 64, other < 64,
    ensures ((1u64 << from) & (1u64 << other) != 0) == (from == other),
{
    assert((from < 64 && other < 64) ==>
        (((1u64 << from) & (1u64 << other) != 0) == (from == other))) by(bit_vector);
}

/// #806 bitset representation: OR preserves every old mark and adds its target.
pub(super) proof fn added_bit(mask: u64, target: u64, other: u64)
    requires target < 64, other < 64,
    ensures ((mask | (1u64 << target)) & (1u64 << other) != 0)
        == (mask & (1u64 << other) != 0 || target == other),
{
    assert((target < 64 && other < 64) ==>
        (((mask | (1u64 << target)) & (1u64 << other) != 0)
            == (mask & (1u64 << other) != 0 || target == other))) by(bit_vector);
}

/// #806: the real initial bitset contains only the source, which reaches itself.
pub(super) proof fn initial_mask(w: World, from: NodeId)
    requires from.0 < w.nodes@.len() <= 64,
    ensures sound_mask(w, from, 1u64 << from.0),
{
    reveal(marked); reveal(reaches);
    assert forall|n: NodeId| marked(1u64 << from.0).contains(n)
        implies n == from by { singleton_bit(from.0, n.0); }
    singleton_bit(from.0, from.0);
}

/// #806: following one active production declaration preserves reachability.
pub(super) proof fn follow_edge(w: World, from: NodeId, k: int)
    requires 0 <= k < w.dependencies@.len(), active(w, w.dependencies@[k]),
        reaches(w, from, w.dependencies@[k].node),
    ensures reaches(w, from, w.dependencies@[k].target.request.unwrap()),
{
    reveal(reaches);
    assert forall|cut: ISet<NodeId>| closed(w, cut) && cut.contains(from)
        implies cut.contains(w.dependencies@[k].target.request.unwrap()) by {
        assert(cut.contains(w.dependencies@[k].node));
    }
}

/// #806: expose one actual marked root's proven reachability.
pub(super) proof fn marked_reachable(w: World, from: NodeId, mask: u64, n: NodeId)
    requires sound_mask(w, from, mask), n.0 < 64, mask & (1u64 << n.0) != 0,
    ensures reaches(w, from, n),
{
    reveal(marked);
}

/// #806: one production OR is exactly set insertion, including preservation
/// of the entire scanned prefix of declarations.
pub(super) proof fn add_mask(w: World, from: NodeId, mask: u64, target: NodeId)
    requires sound_mask(w, from, mask), target.0 < w.nodes@.len() <= 64,
        reaches(w, from, target),
    ensures sound_mask(w, from, mask | (1u64 << target.0)),
        marked(mask | (1u64 << target.0)) == marked(mask).insert(target),
{
    reveal(marked);
    assert forall|n: NodeId| #[trigger] marked(mask | (1u64 << target.0)).contains(n)
        == marked(mask).insert(target).contains(n) by {
        if n.0 < 64 { added_bit(mask, target.0, n.0); }
    }
    assert(marked(mask | (1u64 << target.0)) =~= marked(mask).insert(target));
}

/// #806: a fixed point excluding the target is a concrete separating graph cut.
pub(super) proof fn fixed_mask_excludes(w: World, from: NodeId, to: NodeId, mask: u64)
    requires from.0 < 64, to.0 < 64, sound_mask(w, from, mask),
        mask & (1u64 << to.0) == 0,
        closed(w, marked(mask)),
    ensures !reaches(w, from, to),
{
    reveal(marked); reveal(reaches);
}

/// #806: every active successor edge is either an old edge or the new declaration.
pub(super) open spec fn extension(before: World, after: World, from: NodeId, to: NodeId) -> bool {
    forall|k: int| 0 <= k < after.dependencies@.len() && active(after, after.dependencies@[k])
        ==> (after.dependencies@[k].node == from && after.dependencies@[k].target.request == Some(to))
            || exists|j: int| 0 <= j < before.dependencies@.len() && active(before, before.dependencies@[j])
                && before.dependencies@[j].node == after.dependencies@[k].node
                && before.dependencies@[j].target.request == after.dependencies@[k].target.request
}

/// #806: a prior cut also closes the extended graph when the new edge stays
/// inside it or begins outside it. Edge deletion is allowed.
pub(super) proof fn lift_cut(before: World, after: World, from: NodeId, to: NodeId, cut: ISet<NodeId>)
    requires extension(before, after, from, to), closed(before, cut),
        !cut.contains(from) || cut.contains(to),
    ensures closed(after, cut),
{
    assert forall|k: int| 0 <= k < after.dependencies@.len() && active(after, after.dependencies@[k])
        && cut.contains(after.dependencies@[k].node)
        implies cut.contains(after.dependencies@[k].target.request.unwrap()) by {
        let d = after.dependencies@[k];
        if !(d.node == from && d.target.request == Some(to)) {
            let j = choose|j: int| 0 <= j < before.dependencies@.len() && active(before, before.dependencies@[j])
                && before.dependencies@[j].node == d.node && before.dependencies@[j].target.request == d.target.request;
            assert(cut.contains(before.dependencies@[j].target.request.unwrap()));
        }
    }
}

/// #806: graph cuts are closed under union and intersection, by actual edges.
pub(super) proof fn combine_cuts(w: World, left: ISet<NodeId>, right: ISet<NodeId>)
    requires closed(w, left), closed(w, right),
    ensures closed(w, left.union(right)), closed(w, left.intersect(right)),
{
}

/// State-table cycle rejection / #806: adding an edge with no old return path
/// preserves acyclicity. The proof is independent of root and table counts.
pub(super) proof fn extend_acyclic(before: World, after: World, from: NodeId, to: NodeId)
    requires acyclic(before), !reaches(before, to, from), extension(before, after, from, to),
    ensures acyclic(after),
{
    reveal(reaches);
    let separate = choose|cut: ISet<NodeId>| closed(before, cut) && cut.contains(to) && !cut.contains(from);
    assert forall|k: int| 0 <= k < after.dependencies@.len() && active(after, after.dependencies@[k])
        implies !reaches(after, after.dependencies@[k].target.request.unwrap(), after.dependencies@[k].node) by {
        let d = after.dependencies@[k];
        if d.node == from && d.target.request == Some(to) {
            lift_cut(before, after, from, to, separate);
        } else {
            let j = choose|j: int| 0 <= j < before.dependencies@.len() && active(before, before.dependencies@[j])
                && before.dependencies@[j].node == d.node && before.dependencies@[j].target.request == d.target.request;
            let old_cut = choose|cut: ISet<NodeId>| closed(before, cut)
                && cut.contains(d.target.request.unwrap()) && !cut.contains(d.node);
            if !old_cut.contains(from) || old_cut.contains(to) {
                lift_cut(before, after, from, to, old_cut);
            } else {
                combine_cuts(before, old_cut, separate);
                if separate.contains(d.node) {
                    assert(separate.contains(d.target.request.unwrap()));
                    lift_cut(before, after, from, to, old_cut.intersect(separate));
                } else {
                    lift_cut(before, after, from, to, old_cut.union(separate));
                }
            }
        }
    }
}

/// #806: deleting active edges cannot create a cycle, for arbitrary tables.
pub(super) proof fn restrict_acyclic(before: World, after: World)
    requires acyclic(before),
        forall|k: int| 0 <= k < after.dependencies@.len() && active(after, after.dependencies@[k])
            ==> exists|j: int| 0 <= j < before.dependencies@.len() && active(before, before.dependencies@[j])
                && before.dependencies@[j].node == after.dependencies@[k].node
                && before.dependencies@[j].target.request == after.dependencies@[k].target.request,
    ensures acyclic(after),
{
    reveal(reaches);
    assert forall|k: int| 0 <= k < after.dependencies@.len() && active(after, after.dependencies@[k])
        implies !reaches(after, after.dependencies@[k].target.request.unwrap(), after.dependencies@[k].node) by {
        let d = after.dependencies@[k];
        let j = choose|j: int| 0 <= j < before.dependencies@.len() && active(before, before.dependencies@[j])
            && before.dependencies@[j].node == d.node && before.dependencies@[j].target.request == d.target.request;
        let cut = choose|cut: ISet<NodeId>| closed(before, cut)
            && cut.contains(d.target.request.unwrap()) && !cut.contains(d.node);
        assert forall|k: int| 0 <= k < after.dependencies@.len() && active(after, after.dependencies@[k])
            && cut.contains(after.dependencies@[k].node)
            implies cut.contains(after.dependencies@[k].target.request.unwrap()) by {
            let old = choose|j: int| 0 <= j < before.dependencies@.len() && active(before, before.dependencies@[j])
                && before.dependencies@[j].node == after.dependencies@[k].node
                && before.dependencies@[j].target.request == after.dependencies@[k].target.request;
        }
        assert(closed(after, cut));
    }
}

/// #806: the real table update plus the node-phase frame adds at most its one
/// candidate edge. Retained rows keep their indices and graph interpretation.
pub(super) proof fn table_extension(before: World, after: World, i: int,
    existing: Option<usize>, d: Dependency)
    requires dependencies::dependency_frame(before, after, i),
        0 <= i < before.nodes@.len(), dependencies::edges_indexable(before),
        d.node.0 < before.nodes@.len(),
        match d.target.request { None => true, Some(n) => n.0 < before.nodes@.len() },
        match existing { None => true, Some(k) => k < before.dependencies@.len() },
        after.dependencies@ == match existing {
            None => before.dependencies@.push(d), Some(k) => before.dependencies@.update(k as int, d) },
    ensures dependencies::edges_indexable(after), extension(before, after, d.node, d.target.request.unwrap()),
        !active(after, d) ==> (forall|k: int| 0 <= k < after.dependencies@.len() && active(after, after.dependencies@[k])
            ==> exists|j: int| 0 <= j < before.dependencies@.len() && active(before, before.dependencies@[j])
                && before.dependencies@[j].node == after.dependencies@[k].node
                && before.dependencies@[j].target.request == after.dependencies@[k].target.request),
{
    reveal(dependencies::dependency_frame); dependencies::indexable_definition(before); dependencies::indexable_definition(after);
    assert forall|k: int| 0 <= k < after.dependencies@.len()
        implies after.dependencies@[k] == d || (k < before.dependencies@.len()
            && after.dependencies@[k] == before.dependencies@[k]) by {}
    assert forall|k: int| 0 <= k < after.dependencies@.len() && active(after, after.dependencies@[k])
        implies (active(after, d) && after.dependencies@[k] == d)
            || (k < before.dependencies@.len() && after.dependencies@[k] == before.dependencies@[k]
                && active(before, before.dependencies@[k])) by {
        if after.dependencies@[k] != d {
            let owner = after.dependencies@[k].node.0 as int;
            assert(after.nodes@[owner].phase == before.nodes@[owner].phase);
        }
    }
}

/// State-table cycle rejection / #806: compose a prepared candidate and the
/// actual record_dependency frame. No declaration-count bound is imposed.
pub(super) proof fn record_preserves_acyclic(before: World, after: World, i: int,
    existing: Option<usize>, d: Dependency)
    requires dependencies::dependency_frame(before, after, i),
        0 <= i < before.nodes@.len() <= 64, dependencies::edges_indexable(before), acyclic(before),
        d.node.0 < before.nodes@.len(),
        match d.target.request { None => true, Some(n) => n.0 < before.nodes@.len() },
        match existing { None => true, Some(k) => k < before.dependencies@.len() },
        after.dependencies@ == match existing {
            None => before.dependencies@.push(d), Some(k) => before.dependencies@.update(k as int, d) },
        !d.rejected && d.status == DependencyStatus::Pending && d.target.request.is_some()
            ==> !reaches(before, d.target.request.unwrap(), d.node),
    ensures acyclic(after), dependencies::edges_indexable(after),
{
    table_extension(before, after, i, existing, d);
    if active(after, d) {
        extend_acyclic(before, after, d.node, d.target.request.unwrap());
    } else { restrict_acyclic(before, after); }
}
}
