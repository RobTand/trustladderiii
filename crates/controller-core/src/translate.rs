//! Delivery outcome receipt admission and pushed-head plan preservation.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(not(verus_keep_ghost))]
macro_rules! scan {
    (($condition:expr) [$($contract:tt)*] $body:block) => { while $condition $body };
}
#[cfg(verus_keep_ghost)]
macro_rules! scan {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! { while $condition $($contract)* $body }
    };
}

/// Delivery outcome admitted by its recorded-receipt guard. The shell consumes
/// this value; callers construct it only through `begin` at the receipt read.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct Translation {
    receipt: (),
}

/// S-once: preserve the recorded-receipt early return on
/// f5b0ba2f, lib.rs:2846-2850. #751/#760 require its original sampling position.
/// This is preserved existing dedup behavior, not a new state-table rule.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result.is_some() == !recorded,
))]
pub fn begin(recorded: bool) -> Option<Translation> {
    if recorded {
        None
    } else {
        Some(Translation { receipt: () })
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitRow {
    pub is_commit: bool,
    pub id: i64,
}

/// Push by someone other than TL3; docs/state-table.md:667 on cdacce8f.
/// PushByOther reads the previous SHA from the first matching Commit entry.
/// Preserve the old cast and absence of a scope filter, even for malformed rows.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    match (head, result) {
        (Some(head), Some(index)) => index < rows.len() && rows[index as int].is_commit && rows[index as int].id == head &&
            forall|i: int| 0 <= i < index ==> !(rows[i].is_commit && rows[i].id == head),
        (Some(head), None) => forall|i: int| 0 <= i < rows.len() ==> !(rows[i].is_commit && rows[i].id == head),
        (None, _) => result.is_none(),
    },
))]
#[allow(
    clippy::question_mark,
    reason = "Keep explicit Option control flow in the Verus loop proof."
)]
pub fn previous_commit(rows: &[CommitRow], head: Option<i64>) -> Option<usize> {
    let target = match head {
        Some(head) => head,
        None => return None,
    };
    let mut index = 0;
    scan! { (index < rows.len()) [
        invariant index <= rows.len(), head == Some(target),
            forall|i: int| 0 <= i < index ==> !(rows[i].is_commit && rows[i].id == target),
        decreases rows.len() - index,
    ] {
        if rows[index].is_commit && rows[index].id == target { return Some(index); }
        index += 1;
    } }
    None
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Debug, PartialEq, Eq)]
pub struct CarriedPlan<H, P> {
    pub node: u64,
    pub head: H,
    pub plan: P,
}

/// Push by someone other than TL3; docs/state-table.md:667 on cdacce8f.
/// Carry an available check plan to the pushed head, preserving every field.
/// An absent previous plan produces no check_plans row.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    match (plan, result) {
        (None, None) => true,
        (Some(plan), Some(carried)) => carried.node == node && carried.head == head && carried.plan == plan,
        _ => false,
    },
))]
pub fn carry_plan<H, P>(node: u64, head: H, plan: Option<P>) -> Option<CarriedPlan<H, P>> {
    match plan {
        Some(plan) => Some(CarriedPlan { node, head, plan }),
        None => None,
    }
}

/// Delivery evidence applies only to the currently interned head.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == (current == Some(interned)),
))]
pub fn head_current(interned: u64, current: Option<u64>) -> bool {
    Some(interned) == current
}

/// Q52 / #447; docs/state-table.md:542,545,557 on cdacce8f.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == (enqueue && !cancelling),
))]
pub fn enqueue_pending(enqueue: bool, cancelling: bool) -> bool {
    enqueue && !cancelling
}
