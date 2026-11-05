//! #806: separate selected-node incident frames from executable world updates.
use super::*;

verus! {
/// #806 incident rows: the exact incident update preserves the node frame and shape.
pub(super) proof fn incident_frame(before: Node, after: Node, needs_rob: bool)
    requires after == retirement::incident(before, needs_rob),
        holding(before.phase) ==> before.generation < u64::MAX,
    ensures after.repo == before.repo, after.footprint == before.footprint,
        settle_shape(before) ==> settle_shape(after),
        after.generation as int == before.generation as int
            + if holding(before.phase) { 1int } else { 0int },
        after.next_seq == before.next_seq, after.credit == before.credit,
        after.question == before.question,
        after.waiting_incident, after.incident_needs_rob == needs_rob,
        after.ticket.is_none(),
        after.run_failures == before.run_failures,
        after.run_incidents == before.run_incidents,
        after.fx_failures == before.fx_failures,
        after.fx_incidents == before.fx_incidents,
        after.last_cmd == before.last_cmd,
{
    reveal(retirement::incident);
}

/// #806 incident rows: a resumed holding phase releases its slot and starts a generation.
pub(super) proof fn resumed_result(before: Node, after: Node, needs_rob: bool, r: Resume)
    requires Some(r) == match before.phase {
        Phase::Making => Some(Resume::Making),
        Phase::Checking => Some(Resume::Checking),
        Phase::Reviewing => Some(Resume::Reviewing),
        Phase::Publishing | Phase::Queued => Some(Resume::Publishing),
        Phase::Rebasing => Some(Resume::Rebasing),
        _ => None,
    },
        after == retirement::phased(Node { waiting_incident: true,
            incident_needs_rob: needs_rob, ticket: None, holds: false, ..before }, Phase::Ready(r)),
    ensures after == retirement::incident(before, needs_rob),
{
    reveal(retirement::incident);
}

/// #806 incident rows: a nonholding phase only records its incident wait.
pub(super) proof fn waiting_result(before: Node, after: Node, needs_rob: bool)
    requires !holding(before.phase),
        after == (Node { waiting_incident: true, incident_needs_rob: needs_rob,
            ticket: None, ..before }),
    ensures after == retirement::incident(before, needs_rob),
{
    reveal(retirement::incident);
}

}
