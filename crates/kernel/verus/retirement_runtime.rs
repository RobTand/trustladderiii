//! #806: proof-only consequences of exact production retry and incident updates.
use super::*;
verus! {
/// #806 / Q50: the incident guard stutters exactly.
pub(super) proof fn upstream_noop(n: Node)
    requires n.phase != Phase::Queued || n.waiting_incident,
    ensures retirement::fact_node(n, Fact::UpstreamStalled) == n,
{ reveal(retirement::fact_node); }
/// #806 / Q50: the actual saturated counter update precedes the real incident helper.
pub(super) proof fn upstream_result(n: Node)
    requires n.phase == Phase::Queued, !n.waiting_incident,
    ensures retirement::fact_node(n, Fact::UpstreamStalled) == {
        let count = if n.run_incidents < 2 { (n.run_incidents + 1) as u8 } else { 2u8 };
        retirement::incident(Node { run_incidents: count, ..n }, count >= 2)
    },
{ reveal(retirement::fact_node); }
}
