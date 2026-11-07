//! Finite-path facts consumed by the production dependency traversals.
use super::*;

verus! {
pub(super) proof fn reflexive(w: World, node: NodeId)
    requires node.0 < w.nodes@.len(),
    ensures reachable(w, node, node),
{
    reveal(path);
    reveal(reachable);
    let route = seq![node];
    assert(path(w, route));
}

pub(super) proof fn extend(w: World, from: NodeId, owner: NodeId, target: NodeId)
    requires reachable(w, from, owner), active_edge(w, owner, target),
    ensures reachable(w, from, target),
{
    reveal(path);
    reveal(reachable);
    reveal(active_edge);
    let route = choose|route: Seq<NodeId>| path(w, route)
        && route[0] == from && route[route.len() - 1] == owner;
    let next = route.push(target);
    assert forall|i: int| 0 <= i < next.len() - 1
        implies active_edge(w, next[i], next[i + 1]) by {
        if i < route.len() - 1 {
            assert(next[i] == route[i]);
            assert(next[i + 1] == route[i + 1]);
        }
    }
    assert(path(w, next));
}

pub(super) proof fn path_in_cut(w: World, route: Seq<NodeId>, cut: vstd::iset::ISet<NodeId>, end: int)
    requires path(w, route), 0 <= end < route.len(), cut.contains(route[0]),
        forall|a: NodeId, b: NodeId| cut.contains(a) && active_edge(w, a, b)
            ==> cut.contains(b),
    ensures cut.contains(route[end]),
    decreases end,
{
    reveal(path);
    if end > 0 {
        path_in_cut(w, route, cut, end - 1);
        assert(active_edge(w, route[end - 1], route[end]));
    }
}

pub(super) proof fn cut_excludes(w: World, from: NodeId, to: NodeId, cut: vstd::iset::ISet<NodeId>)
    requires cut.contains(from), !cut.contains(to),
        forall|a: NodeId, b: NodeId| cut.contains(a) && active_edge(w, a, b)
            ==> cut.contains(b),
    ensures !reachable(w, from, to),
{
    reveal(reachable);
    reveal(path);
    if reachable(w, from, to) {
        let route = choose|route: Seq<NodeId>| path(w, route)
            && route[0] == from && route[route.len() - 1] == to;
        path_in_cut(w, route, cut, route.len() - 1);
    }
}

pub(super) proof fn singleton_bit(from: u64, index: u64)
    requires from < 64, index < 64,
    ensures ((1u64 << from) & (1u64 << index) != 0) == (from == index),
{
    assert(((1u64 << from) & (1u64 << index) != 0) == (from == index)) by(bit_vector)
        requires from < 64, index < 64;
}

pub(super) proof fn union_bit(seen: u64, target: u64, index: u64)
    requires target < 64, index < 64,
    ensures ((seen | (1u64 << target)) & (1u64 << index) != 0)
        == (seen & (1u64 << index) != 0 || target == index),
{
    assert(((seen | (1u64 << target)) & (1u64 << index) != 0)
        == (seen & (1u64 << index) != 0 || target == index)) by(bit_vector)
        requires target < 64, index < 64;
}
}
