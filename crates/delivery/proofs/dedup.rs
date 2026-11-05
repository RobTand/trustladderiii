//! State-table messages: at least once, then delete extra matching copies.
#[path = "../src/upstream.rs"] mod upstream;
use upstream::*;
use vstd::prelude::*;
verus! {
/// State-table messages: one complete successful reconciliation after quiescence
/// retains exactly one copy. Each DELETE is separate and may precede a crash.
/// All finite u64 copy counts, no trace bound and no unrolling bound.
fn deduplicate_after_quiescence(before: Upstream) -> (out: Upstream)
    requires before.copies > 0, before.incorrect == 0, before.outstanding == 0,
    ensures out.copies == 1, out.found, out.outstanding == 0,
{
    let mut current = read(before,Class::Deduplicated);
    while current.repair_debt > 0
        invariant current.copies == current.repair_debt + 1,
            current.outstanding == 0, current.incorrect == 0, current.found,
        decreases current.repair_debt,
    {
        current = repair_one(current,Class::Deduplicated);
    }
    current
}
/// State-table messages / F3: an empty look, retry, partial cleanup, restart,
/// then a zombie copy is handled by the next full matching-marker cleanup.
fn dedup_late_witness() {
    let initial = Upstream {copies:0,incorrect:0,outstanding:0,repair_debt:0,found:false};
    let old = issue(initial);
    let absent = read(lose_response(old),Class::Deduplicated);
    let retry = issue(restart(absent));
    let first = land(retry,Class::Deduplicated,true);
    let confirmed_read = read(first,Class::Deduplicated);
    assert(confirmed_read.found && confirmed_read.copies >= 1);
    let zombie = land(restart(confirmed_read),Class::Deduplicated,true);
    assert(zombie.copies == 2);
    let fixed = deduplicate_after_quiescence(zombie);
    assert(fixed.copies == 1);
}
}
fn main() {}
