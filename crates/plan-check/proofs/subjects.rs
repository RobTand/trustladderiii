#[path = "../src/subjects/decision.rs"]
mod decision;
use vstd::prelude::*;
verus! {
/// #820 subject applicability: enclosing definitions compose at `::` boundaries.
/// Arbitrary-length sequences, with no input truncation or bounded search.
proof fn transitive(a: Seq<u8>, b: Seq<u8>, c: Seq<u8>)
    requires decision::covered(a,b), decision::covered(b,c),
    ensures decision::covered(a,c),
{
    assert forall|i:int| 0<=i<a.len() implies a[i]==c[i] by {
        assert(a[i]==b[i]); assert(b[i]==c[i]);
    }
    if a.len()==b.len() {
        if b.len()!=c.len() {assert(c[a.len() as int]==58);assert(c[a.len() as int+1]==58);}
    } else {
        assert(b[a.len() as int]==58);assert(b[a.len() as int+1]==58);
        assert(c[a.len() as int]==b[a.len() as int]);assert(c[a.len() as int+1]==b[a.len() as int+1]);
    }
}
/// #820 reachability of equality, enclosing match, rejected prefix and asymmetry.
fn witnesses() {
    let equal=decision::covers(&[97], &[97]); assert(equal);
    let nested=decision::covers(&[97], &[97,58,58,98]); assert(nested);
    let wrong=decision::covers(&[97], &[97,98]); assert(!wrong);
    let reverse=decision::covers(&[97,58,58,98], &[97]); assert(!reverse);
}
}
fn main() {}
