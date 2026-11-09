use crate::translate::*;
use vstd::prelude::*;
verus! {
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
}
