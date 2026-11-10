//! K5 DEV ONLY (fv-k5-int-dev-1110), CONTRACTS.md v2.20: TRUSTED-STUB of a checker export (owner checker).
//! An owner with an undelivered prerequisite is a waiting owner under the complete dependency acceptance
//! (row_ok's undelivered-row clauses). Assembly uses it with E2's N4 rows for R28. Never lands as a stub.
use super::*;

verus! {
#[verifier::external_body]
pub(super) proof fn waiting_owner_ok(w: World, i: int)
    requires dependencies::rules_ok(w), 0 <= i < w.nodes@.len(),
        !dependencies::prerequisites_delivered(w, NodeId(i as u64)),
    ensures dependencies::owner_waiting_ok(w.nodes@[i]),
{
}

}
