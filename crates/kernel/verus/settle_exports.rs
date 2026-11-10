//! K5 DEV ONLY (fv-k5-int-dev-1110): frozen export E6 for `settling::settle`. Owner OPEN
//! (src/settling.rs has no arm lease; CONTRACTS.md OQ-1). TRUSTED: settle ensures it through a
//! trusted wrapper. Consumers: assembly G6, liveness G22.
use super::*;

verus! {
/// One opaque bundle; consumers `reveal(settle_exports)`.
#[verifier::opaque]
pub(super) open spec fn settle_exports(before: World, after: World) -> bool {
    // E6.1 Settling keeps the world one the checker accepts (REQUIREMENT R6).
    ((checking::accepted(before) && before.gates@.len() <= u64::MAX) ==> checking::accepted(after))
    // E6.2 When settling stops, no unobstructed oldest waiter still has a free, non-overlapping slot (REQUIREMENT R12).
    && (forall|i: int| 0 <= i < after.nodes@.len()
        && advance_family_post::ready_unobstructed(#[trigger] after.nodes@[i]) && oldest_waiter(after, i)
        ==> !scheduler::slot_spec(after, i))
    // E6.3 An unobstructed waiter to make a change keeps its place in line or ends holding a slot (REQUIREMENT R13).
    && (forall|i: int| 0 <= i < before.nodes@.len()
        && advance_family_post::ready_unobstructed(#[trigger] before.nodes@[i])
        && before.nodes@[i].phase == Phase::Ready(Resume::Making)
        ==> (after.nodes@[i].phase == before.nodes@[i].phase && after.nodes@[i].ticket == before.nodes@[i].ticket)
            || (after.nodes@[i].holds && after.nodes@[i].ticket.is_none()))
}

}
