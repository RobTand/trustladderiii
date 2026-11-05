use crate::translate_worker::*;
use vstd::prelude::*;
verus! {
/// Base docs/state-table.md:139, five finite admitted roles plus rejected kind.
/// Concrete witnesses bound only these examples, not the total role proof.
pub fn role_witness() {
    let w6 = valid_role(Phase::Ingesting, Role::Ingest);
    assert(w6);
    let w7 = valid_role(Phase::Auditing, Role::Audit);
    assert(w7);
    let w8 = valid_role(Phase::Planning, Role::Planner);
    assert(w8);
    let w9 = valid_role(Phase::Making, Role::Maker);
    assert(w9);
    let w10 = valid_role(Phase::Reviewing, Role::Reviewer);
    assert(w10);
    let w11 = valid_role(Phase::Other, Role::Maker);
    assert(!w11);
    let w12 = valid_role(Phase::Reviewing, Role::Writer);
    assert(!w12);
    let w13 = valid_role(Phase::Reviewing, Role::Classifier);
    assert(!w13);
    let w14 = valid_role(Phase::Reviewing, Role::Postmortem);
    assert(!w14);
    let w15 = valid_role(Phase::Reviewing, Role::Unknown);
    assert(!w15);
}
/// Base docs/state-table.md:139, present/absent/mismatched generations.
pub fn failure_witness() {
    let w19 = worker_failed(Some(7),7,7);
    assert(w19 == Failure::RunFailed);
    let w20 = worker_failed(None,7,7);
    assert(w20 == Failure::Ignore);
    let w21 = worker_failed(Some(6),7,7);
    assert(w21 == Failure::Ignore);
    let w22 = worker_failed(Some(7),6,7);
    assert(w22 == Failure::Ignore);
}
/// Base docs/state-table.md:485; accepted and each failed self-check condition.
pub fn maker_witness() {
    let w26 = maker_self_check(true,true,true);
    assert(w26);
    let w27 = maker_self_check(false,true,true);
    assert(!w27);
    let w28 = maker_self_check(true,false,true);
    assert(!w28);
    let w29 = maker_self_check(true,true,false);
    assert(!w29);
}
/// Preserved existing behaviour (no rule). Exact fit, exhausted first report,
/// count cutoff, zero-length reports and a shorter report behind a rejected one.
pub fn budget_witness() {
    reveal_with_fuel(spent, 5);
    let lengths = vec![3usize,7usize,1usize];
    let w35 = findings_prefix(&lengths,20,10);
    assert(w35 == 2);
    let w36 = findings_prefix(&lengths,20,2);
    assert(w36 == 0);
    let w37 = findings_prefix(&lengths,1,100);
    assert(w37 == 1);
    let empty = vec![0usize,0usize];
    let w39 = findings_prefix(&empty,20,0);
    assert(w39 == 2);
    let w40 = findings_prefix(&[],20,0);
    assert(w40 == 0);
}
/// Preserved existing behaviour (no rule), five role-specific document routes.
pub fn document_witness() {
    let w44 = document(Role::Ingest);
    assert(w44 == Some(Document::Briefs));
    let w45 = document(Role::Audit);
    assert(w45 == Some(Document::Audits));
    let w46 = document(Role::Planner);
    assert(w46 == Some(Document::Plans));
    let w47 = document(Role::Maker);
    assert(w47 == Some(Document::MakerReplies));
    let w48 = document(Role::Reviewer);
    assert(w48.is_none());
}
/// Base docs/state-table.md:518-519. Concrete approval and changes routes,
/// plus missing-head and unchanged mapper cases.
pub fn review_witness() {
    let approve = review_route(ReviewVerdict::Approve,Some(19));
    assert(approve == ReviewRoute::Approve(19));
    let absent = review_route(ReviewVerdict::Approve,None);
    assert(absent == ReviewRoute::MissingHead);
    let changes = review_route(ReviewVerdict::Changes,Some(19));
    assert(changes == ReviewRoute::Changes);
    let keep = review_route(ReviewVerdict::Other,None);
    assert(keep == ReviewRoute::Keep);
}
}
