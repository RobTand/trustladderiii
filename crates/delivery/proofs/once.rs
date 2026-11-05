//! State-table Once. Conditional on the external original-key deduplication law.
#[path = "../src/upstream.rs"] mod upstream;
use upstream::*;
use vstd::prelude::*;
verus! {
/// State-table Once: induction step for arbitrary schedules and restarts.
/// u64 accounting bound only; no bound on schedule length. External key
/// deduplication is explicit in land, not inferred from a local receipt.
fn once_application_is_idempotent(before: Upstream)
    requires before.copies <= 1, before.incorrect == 0,
        before.outstanding > 0, before.outstanding < u64::MAX,
{
    let delayed = lose_response(before);
    let restarted = restart(delayed);
    let retried = issue(restarted);
    let first = land(retried, Class::Once, true);
    let second = land(first, Class::Once, true);
    assert(first.copies == 1);
    assert(second.copies == 1);
    assert(second.outstanding == before.outstanding - 1);
}
/// State-table Once / F3: timed-out epoch A lands after restart and retry B.
fn once_late_landing_witness() {
    let empty = Upstream {copies:0,incorrect:0,outstanding:0,repair_debt:0,found:false};
    let old = issue(empty);
    let timed_out = lose_response(old);
    let new = issue(restart(timed_out));
    let new_landed = land(new,Class::Once,true);
    let zombie = land(new_landed,Class::Once,true);
    assert(zombie.copies == 1);
    assert(zombie.outstanding == 0);
}
}
fn main() {}
