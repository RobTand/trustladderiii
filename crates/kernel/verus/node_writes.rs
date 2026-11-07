//! Opaque single-node storage writes and their local projections.
use super::*;

verus! {
#[verifier::opaque]
pub(super) open spec fn write(s: Seq<Node>, i: int, value: Node) -> Seq<Node> {
    s.update(i, value)
}

pub(super) proof fn write_length(s: Seq<Node>, i: int, value: Node)
    requires 0 <= i < s.len(),
    ensures write(s, i, value).len() == s.len(),
{
    reveal(write);
}

pub(super) proof fn write_index(s: Seq<Node>, i: int, value: Node)
    requires 0 <= i < s.len(),
    ensures write(s, i, value)[i] == value,
{
    reveal(write);
}

pub(super) proof fn write_other(s: Seq<Node>, i: int, value: Node)
    requires 0 <= i < s.len(),
    ensures forall|j: int| 0 <= j < s.len() && j != i
        ==> write(s, i, value)[j] == s[j],
{
    reveal(write);
}

pub(super) proof fn write_from_frame(b: Seq<Node>, a: Seq<Node>, i: int)
    requires 0 <= i < b.len(), a.len() == b.len(),
        forall|j: int| 0 <= j < b.len() && j != i ==> a[j] == b[j],
    ensures a == write(b, i, a[i]),
{
    reveal(write);
    assert(a =~= b.update(i, a[i]));
}
}
