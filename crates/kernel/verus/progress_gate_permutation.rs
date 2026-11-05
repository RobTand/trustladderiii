//! #806: interchangeable independent roots preserve full consistency and natural measure.
use super::progress_gate::*;
use super::*;

verus! {
/// #806: permutation invariance of #805's node_total, for any finite sequence.
proof fn node_total_swap(nodes: Seq<Node>, i: int)
    requires 0 <= i < nodes.len(),
    ensures node_total(nodes.update(0, nodes[i]).update(i, nodes[0])) == node_total(nodes),
{
    sum_update(nodes, 0, nodes[i]);
    if i != 0 { sum_update(nodes.update(0, nodes[i]), i, nodes[0]); }
    else { assert(nodes.update(0, nodes[i]).update(i, nodes[0]) =~= nodes); }
}

/// Q30: inline footprint overlap is symmetric in the real checker's fast path.
proof fn inline_symmetric(w: World, a: u64, b: u64)
    ensures footprint::disjoint(w, footprint::inline_footprint(a), footprint::inline_footprint(b))
        == footprint::disjoint(w, footprint::inline_footprint(b), footprint::inline_footprint(a)),
{
    footprint::inline_disjoint(w, a, b); footprint::inline_disjoint(w, b, a);
    assert((a & b) == (b & a)) by (bit_vector);
}

/// #806: map each destination index back to its source during actor relocation.
proof fn swap_safe(before: World, after: World, i: int)
    requires input(before), 0 <= i < 4,
        after == (World { nodes: after.nodes, ..before }),
        after.nodes@ == before.nodes@.update(0, before.nodes@[i]).update(i, before.nodes@[0]),
    ensures input(after), progress_measure(after) == progress_measure(before),
{
    reveal(accepted); reveal(retirement::node_rules);
    reveal_with_fuel(potential_prefix, 5);
    progress::empty_edges(before); progress::empty_edges(after);
    node_total_swap(before.nodes@, i);
    assert forall|a: int| 0 <= a < 4 implies leaf(after.nodes@[a])
        && retirement::node_rules(after, after.nodes@[a]) by {
        let src = if a == 0 { i } else if a == i { 0 } else { a };
        assert(after.nodes@[a] == before.nodes@[src]);
        footprint::storage_frame(before, after, before.nodes@[src].footprint, before.nodes@[src].footprint);
    }
    assert forall|a: int, b: int| 0 <= a < b < 4 implies pair(after, a, b) by {
        let x = if a == 0 { i } else if a == i { 0 } else { a };
        let y = if b == 0 { i } else if b == i { 0 } else { b };
        assert(0 <= x < 4 && 0 <= y < 4 && x != y);
        assert(after.nodes@[a] == before.nodes@[x] && after.nodes@[b] == before.nodes@[y]);
        footprint::storage_frame(before, after, before.nodes@[x].footprint, before.nodes@[y].footprint);
        if x < y { assert(pair(before, x, y)); }
        else {
            assert(pair(before, y, x));
            let xf = choose|f: u64| 0 < f < 16 && before.nodes@[x].footprint == footprint::inline_footprint(f);
            let yf = choose|f: u64| 0 < f < 16 && before.nodes@[y].footprint == footprint::inline_footprint(f);
            inline_symmetric(before, xf, yf);
        }
    }
}
}

kernel_fn! {
/// #806: perform the real actor swap, then run the complete production checker.
pub(super) fn swap_actor(mut w: World, actor: usize) -> (after: World) [
    requires input(w), actor < 4,
    ensures input(after), world_checked(after),
        after.nodes@ == w.nodes@.update(0, w.nodes@[actor as int]).update(actor as int, w.nodes@[0]),
        progress_measure(after) == progress_measure(w),
] {
    kernel_snapshot! { before = w }
    let first = w.nodes[0];
    let selected = w.nodes[actor];
    w.nodes[0] = selected;
    w.nodes[actor] = first;
    kernel_proof! { swap_safe(before@, w, actor as int); input_scope(w); }
    verify_checked(&w);
    w
}
}
