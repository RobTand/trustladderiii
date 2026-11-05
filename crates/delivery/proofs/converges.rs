//! State-table Converges / late requests :753.
#[path = "../src/step.rs"] mod step;
pub use step::*;
#[path = "../src/mirror.rs"] mod mirror;
#[path = "../src/upstream.rs"] mod upstream;
use upstream::*;
use vstd::prelude::*;
verus! {
/// State-table Converges: after quiescence an ordinary pending reconciliation
/// confirms iff the upstream read finds the desired state. Holds preserve an
/// absent Uncertain effect; they never turn absence into confirmation.
fn reconciliation_agrees_with_quiescent_upstream(before: Upstream, held: bool)
    requires before.outstanding == 0, before.incorrect <= before.copies,
{
    let observed = read(before,Class::Converges);
    let result = if observed.found {mirror::Read::Found} else {mirror::Read::Absent};
    let action = mirror::reconciliation(State::Uncertain,result,held,false,false);
    let after = mirror::committed(State::Uncertain,action,Ack::Confirmed);
    assert((after == State::Confirmed) == (before.copies > 0));
}
/// State-table :753 / F3: a late landing after an empty read is visible to the
/// next reconciliation. Restart does not cancel the outstanding HTTP request.
fn late_landing_is_found_on_next_read() {
    let old = issue(Upstream {copies:0,incorrect:0,outstanding:0,repair_debt:0,found:false});
    let absent = read(lose_response(old),Class::Converges);
    assert(!absent.found);
    let landed = land(restart(absent),Class::Converges,true);
    let next = read(landed,Class::Converges);
    assert(next.found);
    assert(next.outstanding == 0);
}
}
fn main() {}
