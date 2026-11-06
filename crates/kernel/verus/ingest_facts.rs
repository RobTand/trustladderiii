//! Ingest bounds and branch-local audit equations.
use super::*;

verus! {
/// Ingest replies: translate the zero-output frame to the dispatcher's output lengths.
pub(super) proof fn output_bound(before: World, after: World, i: int,
    old_len: usize, new_len: usize)
    requires reply_bounds(before, after, i, 0, 0), new_len as int <= old_len as int + 1,
    ensures reply_bounds(before, after, i, old_len, new_len),
{ reveal(reply_bounds); }

/// Auditing: keep the exact node equation hidden from world-frame queries.
#[verifier::opaque]
pub(super) open spec fn audit_post(b: Node, a: Node) -> bool {
    a == {
        let updated = Node { audit_rounds: if b.audit_rounds < 255 {
            (b.audit_rounds + 1) as u8
        } else { 255u8 }, ..b };
        if b.audit_rounds == 0 { retirement::phased(updated, Phase::Ingesting) }
        else { retirement::stopped(Node { review_stop: Some(ReviewStop::AuditDisagreement), ..updated }, Outcome::Stopped) }
    }
}

/// Audit disagreement: hide the unchanged counter frame from phase-update queries.
#[verifier::opaque]
pub(super) open spec fn incremented(b: Node, a: Node) -> bool {
    a == Node { audit_rounds: if b.audit_rounds < u8::MAX {
        (b.audit_rounds + 1) as u8
    } else { u8::MAX }, ..b }
}

/// Auditing: the first disagreement uses the one ingest repair.
pub(super) proof fn audit_first(b: Node, updated: Node, a: Node)
    requires b.audit_rounds == 0, incremented(b, updated),
        a == retirement::phased(updated, Phase::Ingesting),
    ensures audit_post(b, a),
{ reveal(audit_post); reveal(incremented); }

/// Auditing: a spent repair stops without a question or credit change.
pub(super) proof fn audit_spent(b: Node, updated: Node, a: Node)
    requires b.audit_rounds >= 1, incremented(b, updated),
        a == retirement::stopped(Node { review_stop: Some(ReviewStop::AuditDisagreement), ..updated }, Outcome::Stopped),
    ensures audit_post(b, a),
{ reveal(audit_post); reveal(incremented); }

/// Auditing: connect the hidden branch result to the existing public audit contract.
pub(super) proof fn audit_result(b: Node, a: Node)
    requires audit_post(b, a),
    ensures a == port_t860_contracts::audit_node(b),
        (b.phase == Phase::Auditing && b.audit_rounds >= 1 ==>
            a.phase == Phase::Stopping(Outcome::Stopped) && a.question.is_none()
                && a.review_stop == Some(ReviewStop::AuditDisagreement) && a.credit == b.credit),
{ reveal(audit_post); reveal(port_t860_contracts::audit_node); reveal(retirement::stopped); }
}
