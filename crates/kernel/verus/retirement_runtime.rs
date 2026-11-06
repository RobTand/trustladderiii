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
/// Q50 / #806: cancellation precedes the actual failed-round call.
#[verifier::opaque]
pub(super) open spec fn ci_prepared(n: Node) -> Node {
    if n.phase == Phase::Publishing { Node { effect: match n.effect {
        Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
    }, ..n } } else { n }
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

/// #806: cancellation changes no field used by the common fact frame.
pub(super) proof fn ci_frame(before: World, prepared: World, after: World, i: usize,
    ob: Outbox, oa: Outbox)
    requires i < before.nodes@.len(),
        prepared.nodes@ == before.nodes@.update(i as int, ci_prepared(before.nodes@[i as int])),
        prepared.gates@ == before.gates@, prepared.file_sets@ == before.file_sets@,
        prepared.dependencies@ == before.dependencies@, prepared.next_ticket == before.next_ticket,
        prepared.next_question == before.next_question, prepared.fail_fast == before.fail_fast,
        prepared.pool_capacity == before.pool_capacity,
        retirement::fact_frame(prepared, after, i, ob, oa),
    ensures retirement::fact_frame(before, after, i, ob, oa),
{ reveal(ci_prepared); reveal(retirement::fact_frame); }
/// Q50 / Q54 / #806: the exact CI source and W2 failed-round tuple, for arbitrary flags.
#[verifier::opaque]
pub(super) open spec fn ci_post(n: Node, after: Node, infra: bool, fast: bool,
    id: u64, next: u64) -> bool {
    if n.phase == Phase::Queued || (n.phase == Phase::Publishing
        && current_item(n) == Some(Item::Do(EffectKind::Enqueue))) {
        if infra { after == retirement::fact_node(n, Fact::CiFailed { infra: true }) && next == id }
        else { (after, next) == port_review_retry::round(ci_prepared(n), fast, id, ReviewStop::CiFailed) }
    } else { after == n && next == id }
}
/// Q54 / #806: a fast paid CI retry emits no extra output.
#[verifier::opaque]
pub(super) open spec fn ci_output(n: Node, infra: bool, fast: bool,
    before: Outbox, after: Outbox) -> bool {
    !infra && fast && n.round != 0 ==> after.len == before.len
}

/// Q50 / #806: cancellation preserves the source agenda and every callee premise.
pub(super) proof fn ci_inputs(n: Node)
    ensures ci_prepared(n).round == n.round,
        current_item(ci_prepared(n)) == current_item(n),
        node_headroom(ci_prepared(n)) == node_headroom(n),
        arithmetic_consistent(ci_prepared(n)) == arithmetic_consistent(n),
{ reveal(ci_prepared); }
/// Q50 / Q54 / #806: project the neutral exact handler contract into retirement's relation.
pub(super) proof fn ci_retirement(n: Node, after: Node, infra: bool, fast: bool, id: u64, next: u64)
    requires id < u64::MAX, ci_post(n, after, infra, fast, id, next),
    ensures after == retirement_retry::fact_result(n, Fact::CiFailed { infra }, fast, id).0,
        next as int == id as int + if retirement_retry::fact_result(n,
            Fact::CiFailed { infra }, fast, id).1 { 1int } else { 0int },
{
    reveal(ci_post); reveal(retirement_retry::fact_result);
    if !infra && (n.phase == Phase::Queued || (n.phase == Phase::Publishing
        && current_item(n) == Some(Item::Do(EffectKind::Enqueue)))) {
        reveal(ci_prepared);
        retirement_retry_bridge::round(ci_prepared(n), fast, id, ReviewStop::CiFailed);
    }
}

}
