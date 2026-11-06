use crate::translate_checks::*;
use vstd::prelude::*;
verus! {
/// Q45, docs/state-table.md:483,496-500 on 751c00ac. Present and absent adoption.
pub fn preparation_routes() {
    let save = preparation(true, true);
    let unadopted = preparation(false, true);
    let stale = preparation(true, false);
    assert(save == Preparation::SavePlan);
    assert(unadopted == Preparation::Ignore);
    assert(stale == Preparation::Ignore);
}
/// Q42, docs/state-machine.md:109 on 751c00ac; head/optional carry preserves
/// existing behaviour (no rule). Both retarget payload flags have live witnesses.
pub fn plan_routes() {
    let all = plan_update(true, true, true);
    let none = plan_update(true, false, false);
    let stale = plan_update(false, true, true);
    assert(all == PlanUpdate::Retarget { set_reviewed: true, replace_tests: true });
    assert(none == PlanUpdate::Retarget { set_reviewed: false, replace_tests: false });
    assert(stale == PlanUpdate::KeepFixes);
}
/// Q41, docs/state-machine.md:132 on 751c00ac. Pass and persistence guards differ:
/// an old job cannot record, but a same-head pass still reaches the kernel tail.
pub fn checks_routes() {
    let live = checks_reply(Some(u64::MAX), u64::MAX, u64::MAX, true, true);
    let old = checks_reply(None, 0, 1, true, true);
    let stale = checks_reply(Some(1), 1, 1, false, true);
    let failed = checks_reply(Some(0), 0, 0, true, false);
    assert(live.record && live.pass);
    assert(!old.record && old.pass);
    assert(!stale.record && !stale.pass);
    assert(failed.record && !failed.pass);
}
}
