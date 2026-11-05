//! #806: prefix arithmetic for the real executable measure scan.
use super::*;
verus! {
/// #806: every natural node prefix fits whenever the whole sum fits.
pub(super) proof fn node_prefix_bound(nodes: Seq<Node>, end: nat)
    requires end <= nodes.len(),
    ensures node_total(nodes.take(end as int)) <= node_total(nodes),
    decreases nodes.len() - end,
{
    if end < nodes.len() {
        node_prefix_bound(nodes.drop_last(), end);
        assert(nodes.drop_last().take(end as int) == nodes.take(end as int));
    } else { assert(nodes.take(end as int) == nodes); }
}
}
