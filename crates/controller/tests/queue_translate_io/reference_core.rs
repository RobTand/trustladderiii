pub mod translate {
//! Queue-arm residue. Mergify owns admission, CI classification and routing.
//! The shell keeps JSON parsing, reads, commit interning and row writes.
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

/// Equality results at the stored JSON boundary. Repository equality here is
/// exact JSON equality, unlike Repository's case-insensitive domain equality.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PriorFailure {
    pub same_repository: bool,
    pub same_number: bool,
    pub same_head: bool,
    pub infra: bool,
    pub overlapping_test: bool,
}

/// Q58; docs/state-machine.md:789-807 and docs/state-table.md:574-575 on cdacce8f.
/// A prior infrastructure failure repeats exactly on this repo, PR and head
/// with an overlapping failed test. The scan is unbounded; the invariant is
/// the small reference model, and the I/O differential checks its JSON parse.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == (exists|i: int| 0 <= i < rows.len() &&
        rows[i].same_repository && rows[i].same_number && rows[i].same_head &&
        rows[i].infra && rows[i].overlapping_test),
))]
pub fn repeated(rows: &[PriorFailure]) -> bool {
    let mut index = 0;
    scan! { (index < rows.len()) [
        invariant index <= rows.len(),
            forall|i: int| 0 <= i < index ==> !(rows[i].same_repository &&
                rows[i].same_number && rows[i].same_head && rows[i].infra && rows[i].overlapping_test),
        decreases rows.len() - index,
    ] {
        let row = &rows[index];
        if row.same_repository && row.same_number && row.same_head && row.infra && row.overlapping_test {
            return true;
        }
        index += 1;
    } }
    false
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelKind {
    PrMerged { checks_passed: bool },
    CiFailed { infra: bool },
    Conflict,
    PrDequeuedByPerson,
    PrClosedByPerson,
    PushByOther,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Record,
    Skip,
    Kernel(KernelKind),
    Incident,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceKind {
    Setup,
    Missing,
    Blocked,
    Reported,
    Merged,
    PushByOther,
    Other,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncidentAction {
    Setup,
    Missing,
    Blocked,
    Clear,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Record,
    Skip,
    Incident(IncidentAction),
    Kernel(KernelKind),
    InvalidIncident,
    InvalidKernel,
}

/// Q52 / #447; docs/state-table.md:542-545,555-557,578 on cdacce8f.
/// Setup evidence never returns a kernel fact, even with an invalid route pairing.
/// Frozen Mergify routing never constructs those pairings; valid mappings stay intact.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == match route {
        Route::Record => Decision::Record,
        Route::Skip => Decision::Skip,
        Route::Incident => match evidence {
            EvidenceKind::Setup => Decision::Incident(IncidentAction::Setup),
            EvidenceKind::Missing => Decision::Incident(IncidentAction::Missing),
            EvidenceKind::Blocked => Decision::Incident(IncidentAction::Blocked),
            EvidenceKind::Reported => Decision::Incident(IncidentAction::Clear),
            _ => Decision::InvalidIncident,
        },
        Route::Kernel(kernel) => if matches!(evidence, EvidenceKind::Setup | EvidenceKind::Missing | EvidenceKind::Blocked | EvidenceKind::Reported) {
            Decision::InvalidKernel
        } else { match kernel {
            KernelKind::PrMerged { .. } => if evidence == EvidenceKind::Merged { Decision::Kernel(kernel) } else { Decision::InvalidKernel },
            KernelKind::PushByOther => if evidence == EvidenceKind::PushByOther { Decision::Kernel(kernel) } else { Decision::InvalidKernel },
            _ => Decision::Kernel(kernel),
        } },
    },
    route == Route::Incident ==> !(result is Kernel),
    matches!(evidence, EvidenceKind::Setup | EvidenceKind::Missing | EvidenceKind::Blocked | EvidenceKind::Reported) ==> !(result is Kernel),
))]
pub fn dispatch(route: Route, evidence: EvidenceKind) -> Decision {
    match route {
        Route::Record => Decision::Record,
        Route::Skip => Decision::Skip,
        Route::Incident => match evidence {
            EvidenceKind::Setup => Decision::Incident(IncidentAction::Setup),
            EvidenceKind::Missing => Decision::Incident(IncidentAction::Missing),
            EvidenceKind::Blocked => Decision::Incident(IncidentAction::Blocked),
            EvidenceKind::Reported => Decision::Incident(IncidentAction::Clear),
            _ => Decision::InvalidIncident,
        },
        Route::Kernel(kernel) => match kernel {
            _ if matches!(
                evidence,
                EvidenceKind::Setup
                    | EvidenceKind::Missing
                    | EvidenceKind::Blocked
                    | EvidenceKind::Reported
            ) =>
            {
                Decision::InvalidKernel
            }
            KernelKind::PrMerged { .. } if evidence != EvidenceKind::Merged => {
                Decision::InvalidKernel
            }
            KernelKind::PushByOther if evidence != EvidenceKind::PushByOther => {
                Decision::InvalidKernel
            }
            _ => Decision::Kernel(kernel),
        },
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IncidentRow {
    pub own_incident: bool,
    pub open: bool,
}

/// Q52; docs/state-table.md:543,556 on cdacce8f.
/// CheckReported clears the first open row with its own incident ID only.
/// The shell computes the ID from node, head, required context and app.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    match result {
        Some(index) => index < rows.len() && rows[index as int].own_incident && rows[index as int].open &&
            forall|i: int| 0 <= i < index ==> !(rows[i].own_incident && rows[i].open),
        None => forall|i: int| 0 <= i < rows.len() ==> !(rows[i].own_incident && rows[i].open),
    },
))]
pub fn clear_reported(rows: &[IncidentRow]) -> Option<usize> {
    let mut index = 0;
    scan! { (index < rows.len()) [
        invariant index <= rows.len(),
            forall|i: int| 0 <= i < index ==> !(rows[i].own_incident && rows[i].open),
        decreases rows.len() - index,
    ] {
        if rows[index].own_incident && rows[index].open { return Some(index); }
        index += 1;
    } }
    None
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

/// Queue attempt head; docs/state-table.md:540,574-576 on cdacce8f.
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

}
