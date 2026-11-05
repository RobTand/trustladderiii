//! State-machine Q27 :961: all matching approval copies converge identically.
#[path = "../src/step.rs"] mod step;
pub use step::*;
#[path = "../src/mirror.rs"] mod mirror;
#[path = "../src/upstream.rs"] mod upstream;
use upstream::*;
use vstd::prelude::*;
verus! {
/// Q27: after quiescence, a successful full reconciliation PATCHes every visible
/// noncanonical conclusion/output to the original operation's canonical value.
/// Incorrect counts encode BOTH conclusion and output, not conclusion alone.
/// All u64 copy counts; unbounded finite schedules, partial repairs and restarts.
fn approval_converges(before: Upstream) -> (out: Upstream)
    requires before.incorrect <= before.copies, before.outstanding == 0,
    ensures out.incorrect == 0, out.copies == before.copies,
        out.outstanding == 0, out.found == (before.copies > 0),
{
    let mut current = read(before,Class::Approval);
    while current.repair_debt > 0
        invariant current.incorrect == current.repair_debt,
            current.copies == before.copies, current.outstanding == 0,
            current.found == (before.copies > 0),
        decreases current.repair_debt,
    {
        current = repair_one(current,Class::Approval);
    }
    current
}
/// Q27: a Dispatching/Uncertain original attempt returns Pending on empty GET.
/// Without the human authorization row, that recovery path never issues POST.
fn no_ambiguous_retry_without_human(predecessor: bool) {
    let blocked = mirror::approval_dispatch(true,predecessor,false,mirror::Read::Pending);
    assert(blocked != mirror::Action::Send);
    let admitted = mirror::approval_dispatch(true,predecessor,true,mirror::Read::Pending);
    assert(admitted == mirror::Action::Send);
}
/// Q27 / F3: an old POST can land after confirmation and restart. Its differing
/// output is repaired by original key, without acknowledging a new-epoch effect.
fn approval_late_witness() {
    let initial=Upstream {copies:0,incorrect:0,outstanding:0,repair_debt:0,found:false};
    let old=issue(initial);
    let delayed=lose_response(old);
    let retry=issue(restart(delayed));
    let first=land(retry,Class::Approval,true);
    let confirmed=read(first,Class::Approval);
    assert(confirmed.found);
    let zombie=land(restart(confirmed),Class::Approval,false);
    let fixed=approval_converges(zombie);
    assert(fixed.copies == 2);
    assert(fixed.incorrect == 0);
}
}
fn main() {}
