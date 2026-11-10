//! #860: exact audit and admission frames, consumed only by this port.
use super::*;

verus! {
/// Reply schema rows / #860: the existing executable accepts table, opaque to callers.
#[verifier::opaque]
pub(super) open spec fn admissible(w: World, n: Node, reply: Reply) -> bool {
    match (n.phase, reply) {
        (Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }, Reply::ProtectedPaths) => true,
        (Phase::Ingesting, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)) => true,
        (Phase::Auditing, Reply::AuditAgrees | Reply::AuditDisagrees) => true,
        (Phase::Planning { .. }, Reply::PlanLeaf { footprint, .. }) =>
            footprint::valid_footprint(w, footprint),
        (Phase::Planning { .. }, Reply::PlanSplit | Reply::PlanCantMeet
            | Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit) => true,
        (Phase::Making, Reply::MakerDone { .. }) => true,
        (Phase::Making, Reply::MakerNeedsFiles { files, .. }) =>
            footprint::valid_footprint(w, files),
        (Phase::Making, Reply::MakerCantMeet | Reply::MakerOutOfRoom) => true,
        (Phase::Checking, Reply::ChecksDone { report, .. }) => report,
        (Phase::Reviewing, Reply::Approved { head }) =>
            n.head == Some(head) && n.checked == Some(head),
        (Phase::Reviewing, Reply::ChangesRequested | Reply::ReviewerOutOfRoom) => true,
        (Phase::Rebasing, Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges) => true,
        _ => false,
    }
}

/// Q30 / #860: successful completion ends the actual operation and resets failures.
#[verifier::opaque]
pub(super) open spec fn ended(n: Node) -> Node {
    Node { run: None, job: None, pool: false, run_failures: 0, run_incidents: 0,
        effect: match n.effect { Some(e) => Some(InFlight { cancelling: e.cancelling
            || matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }), None => None }, ..n }
}

/// Q30 / #860: hide the exact completion equation from unrelated callers.
#[verifier::opaque]
pub(super) open spec fn ended_post(b: Node, a: Node) -> bool { a == ended(b) }

/// Q50 / #860: hide the exact retry equation from unrelated callers.
#[verifier::opaque]
pub(super) open spec fn failed_post(b: Node, a: Node, bo: Outbox, ao: Outbox) -> bool {
    a == failed_node(b) && (retirement::notices_only(bo) ==> retirement::notices_only(ao))
}

/// Auditing / Q27 #832-A: exactly one ingest repair, then a question-free stop.
#[verifier::opaque]
pub(super) open spec fn audit_node(n: Node) -> Node {
    let updated = Node { audit_rounds: if n.audit_rounds < 255 { (n.audit_rounds + 1) as u8 } else { 255u8 }, ..n };
    if n.audit_rounds == 0 { retirement::phased(updated, Phase::Making) }
    else { retirement::stopped(Node { review_stop: Some(ReviewStop::AuditDisagreement), ..updated }, Outcome::Stopped) }
}

/// Q50 / #860: rejected payloads retain the real retry and incident behaviour.
#[verifier::opaque]
pub(super) open spec fn failed_node(n: Node) -> Node {
    let abandoned = Node { run: None, job: None, pool: false,
        effect: match n.effect { Some(e) => Some(InFlight { cancelling: e.cancelling
            || matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }), None => None }, ..n };
    let retried = Node { generation: (n.generation + 1) as u64,
        pos: if n.phase == Phase::Making { 1u8 } else { 0u8 },
        run_failures: if n.run_failures == 0 { 1u8 } else { 0u8 },
        run_incidents: if n.run_failures == 0 { n.run_incidents }
            else if n.run_incidents < 2 { (n.run_incidents + 1) as u8 } else { 2u8 }, ..abandoned };
    if n.run_failures == 0 { retried } else { retirement::incident(retried, n.run_incidents >= 1) }
}

/// Reply admission / #860: matching ownership, independent of the payload schema.
pub(super) open spec fn active(n: Node, generation: u64) -> bool {
    current_item(n) == Some(Item::Await) && (n.run == Some(generation) || n.job == Some(generation))
}

/// Stopping / #860: hide the ownership-release equation from generic postconditions.
#[verifier::opaque]
pub(super) open spec fn stopping_post(b: Node, a: Node, generation: u64) -> bool {
    a == stopping(b, generation)
}

/// Stopping / #860: late replies release only matching operation ownership.
pub(super) open spec fn stopping(n: Node, generation: u64) -> Node {
    Node { run: if n.run == Some(generation) { None } else { n.run },
        pool: n.run != Some(generation) && n.pool,
        job: if n.job == Some(generation) { None } else { n.job }, ..n }
}

/// Reply admission / #860: exact actor for every original phase and generation.
#[verifier::opaque]
pub(super) open spec fn reply_node(n: Node, generation: u64) -> Node {
    if n.phase is Stopping { stopping(n, generation) }
    else if !active(n, generation) { n }
    else if n.phase == Phase::Auditing { audit_node(ended(n)) }
    else { failed_node(n) }
}

/// Auditing / #860: keep the complete update opaque to unrelated rank queries.
#[verifier::opaque]
pub(super) open spec fn transition_post(b: World, a: World, i: int, reply: Reply,
    bo: Outbox, ao: Outbox) -> bool {
    reply == Reply::AuditDisagrees && b.nodes@[i].phase == Phase::Auditing ==> a.nodes@[i] == audit_node(b.nodes@[i])
        && a.next_question == b.next_question && a.file_sets@ == b.file_sets@ && ao == bo
}

/// Reply admission / #860: complete immutable world frame and exact actor.
#[verifier::opaque]
pub(super) open spec fn reply_post(b: World, a: World, i: int, generation: u64,
    reply: Reply, bo: Outbox, ao: Outbox) -> bool {
    reply == Reply::AuditDisagrees ==> a.nodes@ == b.nodes@.update(i, reply_node(b.nodes@[i], generation))
        && a.gates@ == b.gates@ && a.dependencies@ == b.dependencies@
        && a.pool_capacity == b.pool_capacity && a.fail_fast == b.fail_fast && a.next_ticket == b.next_ticket
        && a.file_sets@ == b.file_sets@ && a.next_question == b.next_question
        && (retirement::notices_only(bo) ==> retirement::notices_only(ao))
        && (b.nodes@[i].phase == Phase::Auditing || !active(b.nodes@[i], generation)
            || b.nodes@[i].phase is Stopping ==> ao == bo)
}

/// #860: the selected apply result is the complete opaque reply frame.
#[verifier::opaque]
pub(super) open spec fn apply_post(b: World, a: World, generation: u64, bo: Outbox, ao: Outbox) -> bool {
    reply_post(b, a, 0, generation, Reply::AuditDisagrees, bo, ao)
}

/// Q54 / #629 / Q27 #832, #832-A: the three permitted flag-off review stops.
/// The plan-check case is code-owned and never includes worker PlanCantMeet.
pub(super) open spec fn permitted_stop(b: Node, n: Node, event: Event, i: int) -> bool {
    match event {
        Event::Ack { node, seq, ack: Ack::BodyPolicyRefused } => node.0 == i
            && match b.effect { Some(e) => e.seq == seq && !e.cancelling && e.kind == EffectKind::OpenPr,
                None => false } && n.review_stop == Some(ReviewStop::PublicationBody),
        Event::Reply { node, generation, reply: Reply::AuditDisagrees } => node.0 == i
            && b.phase == Phase::Auditing && b.audit_rounds >= 1 && active(b, generation)
            && n.review_stop == Some(ReviewStop::AuditDisagreement),
        Event::Reply { node, generation, reply: Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit } =>
            node.0 == i && b.phase is Planning && active(b, generation)
                && n.review_stop == Some(ReviewStop::PlanCantMeet),
        _ => false,
    }
}

/// check_nodes and Q54 / #860: all shared assertions, with the approved rule.
pub(super) open spec fn node_change(b: Node, n: Node, fast: bool, event: Event, i: int) -> bool {
    n.proposed == b.proposed && (b.adopted ==> n.adopted) && n.next_seq >= b.next_seq
        && (b.phase is Closed ==> n.phase == b.phase)
        && n.credit.earned == b.credit.earned && n.credit.merge_credited == b.credit.merge_credited
        && (fast ==> n.attempt <= if b.attempt > 1 { b.attempt } else { 1u8 })
        && (fast ==> b.replanned || !n.replanned)
        && (fast && n.phase is Planning ==> b.phase is Planning || b.phase == Phase::AwaitingRob)
        && n.question == b.question
        && (n.review_stop != b.review_stop ==> b.review_stop.is_none()
            && n.phase == Phase::Stopping(Outcome::Stopped) && n.credit == b.credit && n.question.is_none())
        && (!fast && n.review_stop != b.review_stop ==> permitted_stop(b, n, event, i))
}
}
