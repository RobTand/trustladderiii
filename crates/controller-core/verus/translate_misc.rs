#[path = "../src/translate_misc.rs"]
mod translate_misc;
use translate_misc::*;
use vstd::prelude::*;
verus! {
/// Escape: docs/state-table.md:145; Q15 at state-machine.md:477, 9a658a8e,
/// are classification context. Exact intake eligibility is preserved existing
/// behaviour (no rule), controller/src/lib.rs:3042-3043. All valuations reached.
fn admission_witnesses() {
 let a=admit_comment(true,true); assert(a);
 let b=admit_comment(false,true); assert(!b);
 let c=admit_comment(true,false); assert(!c);
 let d=admit_comment(false,false); assert(!d);
}
/// Escape: preserved existing behaviour (no rule), controller/src/lib.rs:3052-3056
/// at 9a658a8e; ab534876a2. Every retirement/deduplication valuation is reachable.
fn recording_witnesses() {
 let a=record_comment(false,false); assert(a);
 let b=record_comment(true,false); assert(!b);
 let c=record_comment(false,true); assert(!c);
 let d=record_comment(true,true); assert(!d);
}
}
fn main() {}
