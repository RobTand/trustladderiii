use crate::memory_retain::*;
use vstd::prelude::*;
verus! {
/// Preserved f5b0ba2f memory.rs:202,226,255-259; #422 is retention context.
/// Finite witnesses supplement total contracts with no excluded inputs.
pub fn qualification() {
    let a = run_succeeded(true,false,true); assert(a);
    let b = run_succeeded(false,false,true); assert(!b);
    let c = run_succeeded(true,true,true); assert(!c);
    let d = run_succeeded(true,false,false); assert(!d);
    let e = review_eligible(true,true); assert(e);
    let f = review_eligible(false,true); assert(!f);
    let g = review_eligible(true,false); assert(!g);
    let h = head_eligible(true,40,true); assert(h);
    let i = head_eligible(false,40,true); assert(!i);
    let j = head_eligible(true,39,true); assert(!j);
    let k = head_eligible(true,40,false); assert(!k);
}
/// Preserved f5b0ba2f memory.rs:394-406; no rule. Arbitrary prefix, no bound.
/// Replaying one identity cannot select a different operation UUID. Exactly-once
/// retention additionally relies on the remote operation-id uniqueness contract.
pub fn identity(high: u64, low: u64) {
    let first = identity_bits(high, low);
    let replay = identity_bits(high, low);
    assert(first == replay);
    let zero = identity_bits(0,0);
    assert(((0u64 & 0xffffffffffff0fffu64) | 0x5000u64) == 0x5000u64) by(bit_vector);
    assert(zero.0 == 0x5000u64);
    assert(((0u64 & 0x3fffffffffffffffu64) | 0x8000000000000000u64) == 0x8000000000000000u64) by(bit_vector);
    assert(zero.1 == 0x8000000000000000u64);
}
/// Preserved f5b0ba2f memory.rs:266-274,319-324. Exact byte boundary and
/// acknowledgement batch size. Store pages remain LIMIT 64, outbox cap 10000.
pub fn bounds() {
    let empty = record_fits(0); assert(empty);
    let exact = record_fits(32768); assert(exact);
    let over = record_fits(32769); assert(!over);
    let negative = stored_record_fits(-1); assert(negative);
    let stored_exact = stored_record_fits(32768); assert(stored_exact);
    let stored_over = stored_record_fits(32769); assert(!stored_over);
    let one = acknowledged(true,true,true,true,true); assert(one);
    let other = acknowledged(true,true,true,true,false); assert(!other);
}
/// Preserved f5b0ba2f memory.rs:1149-1161; #776 failure-hold context.
/// Covers success, auth, retry, parking and statuses outside HTTP ranges.
pub fn classification() {
    let a = http_action(200); assert(a == Action::Done);
    let b = http_action(299); assert(b == Action::Done);
    let c = http_action(401); assert(c == Action::RejectKey);
    let d = http_action(403); assert(d == Action::RejectKey);
    let e = http_action(408); assert(e == Action::Retry);
    let f = http_action(429); assert(f == Action::Retry);
    let g = http_action(400); assert(g == Action::Park);
    let h = http_action(499); assert(h == Action::Park);
    let i = http_action(500); assert(i == Action::Retry);
    let j = http_action(0); assert(j == Action::Retry);
    let k = http_action(65535); assert(k == Action::Retry);
}
/// Preserved f5b0ba2f memory.rs:319-357; #776 empty-wake context.
/// Every acknowledgement conjunct can independently prevent done.
pub fn completion() {
    let a = acknowledged(true,true,true,true,true); assert(a);
    let b = acknowledged(false,true,true,true,true); assert(!b);
    let c = acknowledged(true,false,true,true,true); assert(!c);
    let d = acknowledged(true,true,false,true,true); assert(!d);
    let e = acknowledged(true,true,true,false,true); assert(!e);
    let f = acknowledged(true,true,true,true,false); assert(!f);
    let g = retain_action(true,None); assert(g == Action::Done);
    let h = retain_action(false,None); assert(h == Action::Retry);
    let i = retain_action(false,Some(200)); assert(i == Action::Retry);
    let j = retain_action(false,Some(401)); assert(j == Action::RejectKey);
    let k = retain_action(false,Some(403)); assert(k == Action::RejectKey);
    let l = retain_action(false,Some(408)); assert(l == Action::Retry);
    let m = retain_action(false,Some(429)); assert(m == Action::Retry);
    let n = retain_action(false,Some(422)); assert(n == Action::Park);
    let o = retain_action(false,Some(503)); assert(o == Action::Retry);
    let p = made_progress(0); assert(!p);
    let q = made_progress(1); assert(q);
}
}
