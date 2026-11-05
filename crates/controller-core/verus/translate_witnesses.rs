use crate::translate::*;
use vstd::prelude::*;
verus! {
/// Q58; docs/state-machine.md:789-807 and docs/state-table.md:574-575, cdacce8f. Two rows
/// witness a qualifying history after a nonqualifying prefix, and empty history.
pub fn repeated_witness() {
    let rows = vec![PriorFailure {same_repository:true,same_number:true,same_head:false,infra:true,overlapping_test:true},
        PriorFailure {same_repository:true,same_number:true,same_head:true,infra:true,overlapping_test:true}];
    let yes = repeated(&rows);
    assert(yes);
    let no = repeated(&[]);
    assert(!no);
}
/// Q52/#447; docs/state-table.md:542-545, cdacce8f. Four setup kinds
/// witness every successful incident action, without a kernel fact.
pub fn setup_witness() {
    let setup = dispatch(Route::Incident, EvidenceKind::Setup);
    let missing = dispatch(Route::Incident, EvidenceKind::Missing);
    let blocked = dispatch(Route::Incident, EvidenceKind::Blocked);
    let clear = dispatch(Route::Incident, EvidenceKind::Reported);
    assert(setup == Decision::Incident(IncidentAction::Setup));
    assert(missing == Decision::Incident(IncidentAction::Missing));
    assert(blocked == Decision::Incident(IncidentAction::Blocked));
    assert(clear == Decision::Incident(IncidentAction::Clear));
}
/// Q52; docs/state-table.md:543,556, cdacce8f. Three rows witness
/// exclusion of another open incident and of this incident's closed row.
pub fn clear_witness() {
    let rows = vec![IncidentRow {own_incident:false,open:true},IncidentRow {own_incident:true,open:false},
        IncidentRow {own_incident:true,open:true}];
    let selected = clear_reported(&rows);
    assert(selected == Some(2usize));
}
/// Push by someone other than TL3; docs/state-table.md:667, cdacce8f. Three entries witness
/// kind filtering and first-match previous-SHA selection, plus absent head.
pub fn previous_witness() {
    let rows = vec![CommitRow {is_commit:false,id:1},CommitRow {is_commit:true,id:1},CommitRow {is_commit:true,id:1}];
    let selected = previous_commit(&rows, Some(1i64));
    assert(selected == Some(1usize));
    let absent = previous_commit(&rows, None);
    assert(absent.is_none());
}
/// Push by someone other than TL3; docs/state-table.md:667, cdacce8f. One plan witnesses all
/// preservation fields with distinct old/new marker values.
pub fn carry_witness() {
    let carried = carry_plan(7u64, 22u64, Some(41u64));
    assert(carried.is_some());
    match carried {
        Some(carried) => assert(carried.node == 7 && carried.head == 22 && carried.plan == 41),
        None => assert(false),
    }
    let absent: Option<CarriedPlan<u64, u64>> = carry_plan(7u64, 22u64, None);
    assert(absent.is_none());
}
/// Q52/#447; docs/state-table.md:542-545,555-557,578, cdacce8f. Four witnesses
/// reach invalid kernel pairings directly, independently of the shell's routing.
pub fn setup_rejects_kernel_witness() {
    let route = Route::Kernel(KernelKind::CiFailed {infra:false});
    let setup = dispatch(route, EvidenceKind::Setup);
    let missing = dispatch(route, EvidenceKind::Missing);
    let blocked = dispatch(route, EvidenceKind::Blocked);
    let reported = dispatch(route, EvidenceKind::Reported);
    assert(setup == Decision::InvalidKernel);
    assert(missing == Decision::InvalidKernel);
    assert(blocked == Decision::InvalidKernel);
    assert(reported == Decision::InvalidKernel);
}

}
