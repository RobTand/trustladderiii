//! Q54 / #440 / #806: the shared node and fail-fast assertions of the retired checks.
use super::*;

verus! {
/// State-table node/output rules and Q54: every selected reply preserves
/// proposal, adoption, effect keys and merge-credit marking. A changed review
/// reason stops without spending or asking; no selected reply starts a retry.
pub(super) open spec fn node_post(before: Node, after: Node) -> bool {
    after.proposed == before.proposed
        && (before.adopted ==> after.adopted)
        && after.next_seq >= before.next_seq
        && (before.phase is Closed ==> after.phase == before.phase)
        && after.credit.earned == before.credit.earned
        && after.credit.merge_credited == before.credit.merge_credited
        && after.attempt <= if before.attempt < 1 { 1u8 } else { before.attempt }
        && (after.replanned ==> before.replanned)
        && (after.phase is Planning ==> before.phase is Planning || before.phase == Phase::AwaitingRob)
        && (match after.question { Some(q) => q.kind == QKind::CantMeet, None => false }
            ==> after.question == before.question)
        && (after.review_stop != before.review_stop ==>
            before.review_stop.is_none() && after.phase == Phase::Stopping(Outcome::Stopped)
                && after.credit == before.credit && after.question.is_none())
}
}
