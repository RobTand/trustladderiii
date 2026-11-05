use crate::review_inputs::{bounds::*, history::*, selection::*};
use vstd::prelude::*;
verus! {
/// Preserved f5b0ba2f review.rs:84-85, :136-140, :167-168. At/over byte
/// cap and zero/exact/over scalar prefix. These examples do not bound proofs.
pub fn bounds_witness() {
    let empty = diff_allowed(0); assert(empty);
    let exact = diff_allowed(524288); assert(exact);
    let over = diff_allowed(524289); assert(!over);
    let p0 = prefix_len(0,1000); assert(p0 == 0);
    let p1 = prefix_len(1000,1000); assert(p1 == 1000);
    let p2 = prefix_len(1001,1000); assert(p2 == 1000);
    let p3 = prefix_len(16001,16000); assert(p3 == 16000);
}
/// Preserved f5b0ba2f review.rs:42-70. Re-review precedence and both ancestry
/// results are reachable; state-table.md:512-514 supplies re-review context.
pub fn history_witness() {
    let a=base_source(false,true,true); assert(a == BaseSource::Check);
    let b=base_source(true,false,true); assert(b == BaseSource::Check);
    let c=base_source(true,true,false); assert(c == BaseSource::Session);
    let d=base_source(true,true,true); assert(d == BaseSource::Reviewed);
    let e=reset_base(true,false); assert(e);
    let f=reset_base(true,true); assert(!f);
    let g=reset_base(false,false); assert(!g);
}
/// Preserved f5b0ba2f review.rs:97-136, :154-160, :189-202. Empty/nonempty
/// stable selection, every forcing key, base/head receipt distinction.
pub fn selection_witness() {
    let a=indices(&[]); assert(a.len() == 0);
    let b=indices(&[true,false,true]);
    assert(b.len() == 2) by {
        assert(b@.contains(0usize)); assert(b@.contains(2usize));
        if b.len() < 2 { assert(false); }
        if b.len() > 2 {
            assert(b[0] == 0); assert(b[1] == 1); assert(b[2] == 2);
            assert(!b@.contains(1usize));
            assert(false);
        }
    }
    assert(b[0] == 0); assert(b[1] == 2);
    let c=forced(true,false,false); assert(c);
    let d=forced(false,true,false); assert(d);
    let e=forced(false,false,true); assert(e);
    let f=forced(false,false,false); assert(!f);
    let g=receipt_tree(true,true,false); assert(g);
    let h=receipt_tree(true,false,true); assert(!h);
    let i=receipt_tree(false,false,true); assert(i);
    let r0=resolves_historical(true,true,HeadOutcome::Passed); assert(r0);
    let r1=resolves_historical(true,true,HeadOutcome::Assertion); assert(r1);
    let r2=resolves_historical(true,true,HeadOutcome::Error); assert(r2);
    let r3=resolves_historical(true,true,HeadOutcome::Other); assert(!r3);
    let r4=resolves_historical(true,false,HeadOutcome::Passed); assert(!r4);
    let r5=resolves_historical(false,true,HeadOutcome::Passed); assert(!r5);
    let j=receipt_tree(false,true,false); assert(!j);
}
}
