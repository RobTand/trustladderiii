//! Reply schema decisions over the actual pre-state's scalar admission fields.
use super::*;

verus! {
pub(super) type Input = (Phase, Option<HeadId>, Option<HeadId>, bool);

/// Reply rows: only footprint-bearing payloads consult the real storage table.
#[verifier::opaque]
pub(super) open spec fn input(w: World, n: Node, reply: Reply) -> Input {
    (n.phase, n.head, n.checked, match reply {
        Reply::PlanLeaf { footprint, .. } | Reply::MakerNeedsFiles { files: footprint, .. } =>
            footprint::valid_footprint(w, footprint),
        _ => true,
    })
}

/// Reply rows: phase, schema and commit identity, independent of World storage.
#[verifier::opaque]
pub(super) open spec fn allows(before: Input, reply: Reply) -> bool {
    match (before.0, reply) {
        (Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }, Reply::ProtectedPaths) => true,
        (Phase::Ingesting, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)) => true,
        (Phase::Auditing, Reply::AuditAgrees | Reply::AuditDisagrees) => true,
        (Phase::Planning { .. }, Reply::PlanLeaf { .. }) => before.3,
        (Phase::Planning { .. }, Reply::PlanSplit | Reply::PlanCantMeet
            | Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit) => true,
        (Phase::Making, Reply::MakerDone { .. }) => true,
        (Phase::Making, Reply::MakerNeedsFiles { .. }) => before.3,
        (Phase::Making, Reply::MakerCantMeet | Reply::MakerOutOfRoom) => true,
        (Phase::Checking, Reply::ChecksDone { report, .. }) => report,
        (Phase::Reviewing, Reply::Approved { head }) =>
            before.1 == Some(head) && before.2 == Some(head),
        (Phase::Reviewing, Reply::ChangesRequested | Reply::ReviewerOutOfRoom) => true,
        (Phase::Rebasing, Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges) => true,
        _ => false,
    }
}

/// Planning/Making rows: recover only the admitted payload's storage premise.
pub(super) proof fn accepted_payload(w: World, n: Node, reply: Reply)
    requires allows(input(w, n, reply), reply),
    ensures match reply {
        Reply::PlanLeaf { footprint, .. } | Reply::MakerNeedsFiles { files: footprint, .. } =>
            footprint::valid_footprint(w, footprint),
        _ => true,
    },
{ reveal(input); reveal(allows); }

/// Auditing row: a wrong-phase disagreement is a rejected operation reply.
pub(super) proof fn audit_phase(w: World, n: Node)
    ensures allows(input(w, n, Reply::AuditDisagrees), Reply::AuditDisagrees)
        == (n.phase == Phase::Auditing),
{ reveal(input); reveal(allows); }

/// Planning review rows: code-owned plan-check replies cannot be schema-rejected.
pub(super) proof fn plan_check_phase(w: World, n: Node, reply: Reply)
    requires matches!(reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
    ensures allows(input(w, n, reply), reply) == (n.phase is Planning),
{ reveal(input); reveal(allows); }

/// Making row: only MakerDone in Making can reach the maker result consumer.
pub(super) proof fn maker_phase(w: World, n: Node, head: HeadId)
    ensures allows(input(w, n, Reply::MakerDone { head }), Reply::MakerDone { head })
        == (n.phase == Phase::Making),
{ reveal(input); reveal(allows); }

/// Successful reply rows: consistency supplies Approved's actual head identity.
pub(super) proof fn successful(w: World, n: Node, reply: Reply)
    requires node_checked(w, n), successful_reply_input(n, reply),
        (reply is PlanLeaf && footprint::footprint_empty(n.footprint)
            ==> footprint::valid_footprint(w, footprint::inline_footprint(1))),
    ensures allows(input(w, n, reply), reply),
{ reveal(input); reveal(allows); }
}
