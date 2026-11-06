use vstd::prelude::*;
use crate::translate_tail::*;
verus! {
/// Base f5b0ba2f docs/state-table.md:507-508. Exact report gating is
/// preserved existing behaviour (no rule). Both failure dimensions reachable.
pub fn check_witness() {
    let n = Some(Node {phase:Phase::Checking,generation:7,run:None,job:Some(7)});
    let outside = finding(n,Input::Reply {generation:7,reply:Reply::ChecksDone {report:true,pass:true,in_footprint:false}},false);
    assert(outside == Finding::Check {pass:true,in_footprint:false});
    let failed = finding(n,Input::Reply {generation:7,reply:Reply::ChecksDone {report:true,pass:false,in_footprint:true}},false);
    assert(failed == Finding::Check {pass:false,in_footprint:true});
    let clean = finding(n,Input::Reply {generation:7,reply:Reply::ChecksDone {report:true,pass:true,in_footprint:true}},false);
    assert(clean == Finding::None);
    let silent = finding(n,Input::Reply {generation:7,reply:Reply::ChecksDone {report:false,pass:false,in_footprint:false}},false);
    assert(silent == Finding::None);
}
/// Base f5b0ba2f docs/state-table.md:519. Detailed-review suppression is
/// preserved existing behaviour (no rule); both branches witnessed.
pub fn review_witness() {
    let n = Some(Node {phase:Phase::Reviewing,generation:7,run:Some(7),job:None});
    let input = Input::Reply {generation:7,reply:Reply::ChangesRequested};
    let fallback = finding(n,input,false);
    let detailed = finding(n,input,true);
    assert(fallback == Finding::Review);
    assert(detailed == Finding::None);
}
/// Base f5b0ba2f docs/state-table.md:139,:489-490. Run/job/current-generation
/// conjunction is preserved existing behaviour (no rule), including max u64.
pub fn maker_witness() {
    let input = Input::Reply {generation:0xffffffffffffffff,reply:Reply::MakerCantMeet};
    let current = finding(Some(Node {phase:Phase::Making,generation:0xffffffffffffffff,run:None,job:Some(0xffffffffffffffff)}),input,true);
    let stale = finding(Some(Node {phase:Phase::Making,generation:0,run:Some(0xffffffffffffffff),job:None}),input,false);
    let unattached = finding(Some(Node {phase:Phase::Making,generation:0xffffffffffffffff,run:None,job:None}),input,false);
    assert(current == Finding::Maker);
    assert(stale == Finding::None);
    assert(unattached == Finding::None);
}
/// Base f5b0ba2f docs/state-table.md:574-575. Recording only code failures
/// in Queued is preserved existing behaviour (no rule); no operation required.
pub fn ci_witness() {
    let n = Some(Node {phase:Phase::Queued,generation:0,run:None,job:None});
    let code = finding(n,Input::CiFailed {infra:false},true);
    let infra = finding(n,Input::CiFailed {infra:true},false);
    let wrong_phase = finding(Some(Node {phase:Phase::Checking,generation:0,run:None,job:None}),Input::CiFailed {infra:false},false);
    assert(code == Finding::Ci);
    assert(infra == Finding::None);
    assert(wrong_phase == Finding::None);
}
/// Base f5b0ba2f lib.rs:3844-3889, preserved existing behaviour (no rule).
/// Missing node, other event/reply and mismatched phase reach no-finding.
pub fn absent_witness() {
    let absent = finding(None,Input::CiFailed {infra:false},false);
    let n = Some(Node {phase:Phase::Other,generation:0,run:Some(0),job:None});
    let other = finding(n,Input::Other,false);
    let reply = finding(n,Input::Reply {generation:0,reply:Reply::Other},false);
    let mismatched = finding(n,Input::Reply {generation:0,reply:Reply::MakerCantMeet},false);
    assert(absent == Finding::None);
    assert(other == Finding::None);
    assert(reply == Finding::None);
    assert(mismatched == Finding::None);
}
}
