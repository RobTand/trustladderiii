//! #806: shared admission relation for real worker and review replies.
use super::*;

verus! {
/// State-table reply admission: the real phase, footprint and head schema checks.
pub(super) open spec fn accepted_reply(w: World, n: Node, reply: Reply) -> bool {
    match (n.phase, reply) {
        (Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }, Reply::ProtectedPaths) => true,
        (Phase::Ingesting, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)) => true,
        (Phase::Auditing, Reply::AuditAgrees | Reply::AuditDisagrees) => true,
        (Phase::Planning { .. }, Reply::PlanLeaf { footprint, .. }) => footprint::valid_footprint(w, footprint),
        (Phase::Planning { .. }, Reply::PlanSplit | Reply::PlanCantMeet
            | Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit) => true,
        (Phase::Making, Reply::MakerDone { .. } | Reply::MakerCantMeet | Reply::MakerOutOfRoom) => true,
        (Phase::Making, Reply::MakerNeedsFiles { files, .. }) => footprint::valid_footprint(w, files),
        (Phase::Checking, Reply::ChecksDone { report, .. }) => report,
        (Phase::Reviewing, Reply::Approved { head }) => n.head == Some(head) && n.checked == Some(head),
        (Phase::Reviewing, Reply::ChangesRequested | Reply::ReviewerOutOfRoom) => true,
        (Phase::Rebasing, Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges) => true,
        _ => false,
    }
}

/// Q30: admission requires a current Await and an existing matching operation.
pub(super) open spec fn active(n: Node, generation: u64) -> bool {
    current_item(n) == Some(Item::Await) && (n.run == Some(generation) || n.job == Some(generation))
}

/// State-table stopping rows: late replies only release matching operation ownership.
pub(super) open spec fn stopping_reply(n: Node, generation: u64) -> Node {
    Node { run: if n.run == Some(generation) { None } else { n.run },
        pool: n.pool && n.run != Some(generation),
        job: if n.job == Some(generation) { None } else { n.job }, ..n }
}

/// State-table successful completion: end_operation precedes every admitted handler.
pub(super) open spec fn ended(n: Node) -> Node {
    Node { run_failures: 0, run_incidents: 0, ..port_review_retry::abandoned(n) }
}

/// #806 slice 1: the review and rebase replies whose exact updates W2 owns.
pub(super) open spec fn selected(reply: Reply) -> bool {
    matches!(reply, Reply::Approved { .. } | Reply::ChangesRequested | Reply::ReviewerOutOfRoom
        | Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges | Reply::RunFailed)
}

/// State-table accepted review/rebase rows: exact real handler result.
#[verifier::opaque]
pub(super) open spec fn transition(n: Node, fail_fast: bool, id: u64, reply: Reply) -> (Node, u64) {
    match reply {
        Reply::Approved { head } => (retirement::phased(Node { approved: Some(head), ..n }, Phase::Publishing), id),
        Reply::ChangesRequested => port_review_retry::round(n, fail_fast, id, ReviewStop::ChangesRequested),
        Reply::ReviewerOutOfRoom => if n.review_restarted {
            port_review_retry::room(n, fail_fast, id, ReviewStop::ReviewerOutOfRoom)
        } else { port_review_retry::rechecked(Node { review_restarted: true, ..n }, Phase::Reviewing, id) },
        Reply::RebaseClean { head } => port_review_retry::rechecked(Node {
            head: Some(head), checked: None,
            review_restarted: n.head == Some(head) && n.review_restarted, ..n }, Phase::Checking, id),
        Reply::RebaseNeedsChanges => if port_review_retry::funded(n) {
            (retirement::phased(port_review_retry::paid(n), Phase::Making), id)
        } else { (port_review_retry::asked(n, id, QKind::OutOfCredit, Cont::Ready(Resume::Making)), (id + 1) as u64) },
        _ => (n, id),
    }
}

/// #806: accepted-handler relation; worker-family clauses are supplied by W1.
#[verifier::opaque]
pub(super) open spec fn transition_post(before: World, after: World, i: int, source: Node,
    reply: Reply, out_before: Outbox, out_after: Outbox) -> bool {
    (selected(reply) ==> after.nodes@[i] == transition(source, before.fail_fast, before.next_question, reply).0
        && after.next_question == transition(source, before.fail_fast, before.next_question, reply).1
        && after.file_sets@ == before.file_sets@
        && (retirement::notices_only(out_before) ==> retirement::notices_only(out_after)))
        && port_replies_contracts::transition_post(before, after, i, source, reply, out_before, out_after)
}

/// #806: the single reply relation forwarded by on_reply and the apply reply arm.
/// Stopping, stale and schema rejection cover every payload and generation.
#[verifier::opaque]
pub(super) open spec fn reply_post(before: World, after: World, i: int, generation: u64,
    reply: Reply, out_before: Outbox, out_after: Outbox) -> bool {
    let n = before.nodes@[i];
    after.nodes@.len() == before.nodes@.len()
        && (forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> after.nodes@[j] == before.nodes@[j])
        && after.gates@ == before.gates@ && after.next_ticket == before.next_ticket
        && after.pool_capacity == before.pool_capacity && after.fail_fast == before.fail_fast
        && after.dependencies@ == before.dependencies@
        && if n.phase is Stopping {
            after.nodes@[i] == stopping_reply(n, generation)
                && after.next_question == before.next_question && after.file_sets@ == before.file_sets@
                && out_after == out_before
        } else if !active(n, generation) {
            after.nodes@[i] == n && after.next_question == before.next_question
                && after.file_sets@ == before.file_sets@ && out_after == out_before
        } else if !accepted_reply(before, n, reply) {
            after.nodes@[i] == port_review_retry::failed_run(n)
                && after.next_question == before.next_question && after.file_sets@ == before.file_sets@
                && (retirement::notices_only(out_before) ==> retirement::notices_only(out_after))
        } else { transition_post(before, after, i, ended(n), reply, out_before, out_after) }
}
}
