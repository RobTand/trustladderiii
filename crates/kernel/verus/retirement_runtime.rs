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

/// Q50 / #806: exact no-op branches, isolated from the executable match.
pub(super) proof fn retry_noop(n: Node, fact: Fact, fast: bool, id: u64)
    requires match fact {
        Fact::CiFailed { .. } => n.phase != Phase::Queued && !(n.phase == Phase::Publishing
            && current_item(n) == Some(Item::Do(EffectKind::Enqueue))),
        Fact::PushByOther { .. } => !retirement_retry::published_source(n),
        _ => false,
    },
    ensures retirement_retry::fact_result(n, fact, fast, id) == (n, false),
{ reveal(retirement_retry::fact_result); }
/// Q50 / #806: infrastructure forwarding does not unfold the paid retry ladder.
pub(super) proof fn ci_infra(n: Node, fast: bool, id: u64)
    requires n.phase == Phase::Queued || (n.phase == Phase::Publishing
        && current_item(n) == Some(Item::Do(EffectKind::Enqueue))),
    ensures retirement_retry::fact_result(n, Fact::CiFailed { infra: true }, fast, id)
        == (retirement::fact_node(n, Fact::CiFailed { infra: true }), false),
{ reveal(retirement_retry::fact_result); }
/// Q50 / #806: cancellation precedes the actual failed-round call.
pub(super) open spec fn ci_prepared(n: Node) -> Node {
    if n.phase == Phase::Publishing { Node { effect: match n.effect {
        Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
    }, ..n } } else { n }
}
/// Q54 / #806: compose W2's exact tuple without expanding it in the real handler.
pub(super) proof fn ci_retry(n: Node, fast: bool, id: u64, after: Node, next: u64)
    requires id < u64::MAX,
        n.phase == Phase::Queued || (n.phase == Phase::Publishing
            && current_item(n) == Some(Item::Do(EffectKind::Enqueue))),
        after == port_review_retry::round(ci_prepared(n), fast, id, ReviewStop::CiFailed).0,
        next == port_review_retry::round(ci_prepared(n), fast, id, ReviewStop::CiFailed).1,
    ensures after == retirement_retry::fact_result(n, Fact::CiFailed { infra: false }, fast, id).0,
        next as int == id as int + if retirement_retry::fact_result(n,
            Fact::CiFailed { infra: false }, fast, id).1 { 1int } else { 0int },
{
    retirement_retry_bridge::round(ci_prepared(n), fast, id, ReviewStop::CiFailed);
    reveal(retirement_retry::fact_result);
}
/// Q50 / #806: the real head updater prepares this exact recheck input.
pub(super) open spec fn outside_prepared(n: Node, head: HeadId) -> Node {
    Node { head: Some(head), checked: None,
        review_restarted: n.head == Some(head) && n.review_restarted, ..n }
}
/// Q50 / #806: compose W2's opaque recheck relation with the actual head update.
pub(super) proof fn outside_retry(n: Node, head: HeadId, fast: bool, id: u64,
    after: Node, next: u64)
    requires id < u64::MAX, retirement_retry::published_source(n),
        port_review_retry::recheck_post(outside_prepared(n, head), after,
            Phase::Ready(Resume::Checking), id, next),
    ensures after == retirement_retry::fact_result(n, Fact::PushByOther { head }, fast, id).0,
        next as int == id as int + if retirement_retry::fact_result(n,
            Fact::PushByOther { head }, fast, id).1 { 1int } else { 0int },
{
    reveal(port_review_retry::recheck_post);
    retirement_retry_bridge::recheck(outside_prepared(n, head), Phase::Ready(Resume::Checking), id);
    reveal(retirement_retry::fact_result);
}
}
