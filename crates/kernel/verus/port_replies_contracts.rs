//! #806: exact frames for the ingest, audit and planning reply rows.
use super::*;

verus! {
/// #806 slice 1: these eight payloads retain arbitrary parameters and generations.
pub(super) open spec fn selected(reply: Reply) -> bool {
    matches!(reply, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)
        | Reply::ProtectedPaths | Reply::AuditAgrees | Reply::AuditDisagrees
        | Reply::PlanLeaf { .. } | Reply::PlanSplit | Reply::PlanCantMeet)
}

/// Ingest admission rows / #806: the five worker payloads admitted by phase.
pub(super) open spec fn ingest_accepted(n: Node, reply: Reply) -> bool {
    match (n.phase, reply) {
        (Phase::Ingesting, Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_)) => true,
        (Phase::Auditing, Reply::AuditAgrees | Reply::AuditDisagrees) => true,
        (Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }, Reply::ProtectedPaths) => true,
        _ => false,
    }
}

/// Ingest rows / #806: map the real early-ending enum to its stopping outcome.
pub(super) open spec fn early_outcome(e: EarlyEnd) -> Outcome {
    match e {
        EarlyEnd::Duplicate => Outcome::Duplicate,
        EarlyEnd::AlreadyDone => Outcome::AlreadyDone,
        EarlyEnd::NotActionable => Outcome::NotActionable,
    }
}

/// Reply admission / #806: successful completion ends only the current operation.
pub(super) open spec fn ended(n: Node) -> Node {
    Node { run: None, job: None, pool: false, run_failures: 0, run_incidents: 0,
        effect: match n.effect {
            Some(e) => Some(InFlight { cancelling: e.cancelling || matches!(e.kind,
                EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }),
            None => None,
        }, ..n }
}

/// Ingest rows / #806: deposit once, then audit with unchanged ledger fields.
#[verifier::opaque]
pub(super) open spec fn ingested(n: Node, size: u8, early_end: Option<EarlyEnd>) -> Node {
    retirement::phased(Node { credit: Credit { ready: true,
        start: if n.credit.ready { n.credit.start } else { (size as u64 + RESERVE) as u64 },
        ..n.credit }, early_end, ..n }, Phase::Auditing)
}

/// Question rows / #806: exact selected-node frame of the real ask_rob body.
pub(super) open spec fn asked(n: Node, id: u64, kind: QKind, cont: Cont, cost: u8) -> Node {
    retirement::phased(Node { holds: false,
        question: Some(Question { id, kind, cont, cost, clarified: false }), ..n }, Phase::AwaitingRob)
}

/// Q54 planner retry / #806: preserve the pre-existing saturating retry counter.
#[verifier::opaque]
pub(super) open spec fn plan_retry_node(n: Node, id: u64, fast: bool, cant_meet: bool) -> Node {
    let retries = if n.plan_retries < u8::MAX { (n.plan_retries + 1) as u8 } else { u8::MAX };
    let incremented = Node { plan_retries: retries, ..n };
    if fast && cant_meet {
        retirement::stopped(Node { review_stop: Some(ReviewStop::PlanCantMeet), ..incremented }, Outcome::Stopped)
    } else if retries < 2 {
        retirement::phased(incremented, n.phase)
    } else {
        asked(Node { plan_retries: 0, ..n }, id, QKind::CantMeet,
            Cont::Planning { must_split: matches!(n.phase, Phase::Planning { must_split: true }) }, 1)
    }
}

/// Ingest/audit/plan rows / #806: exact handler update after end_operation.
#[verifier::opaque]
pub(super) open spec fn transitioned(n: Node, id: u64, fast: bool, reply: Reply) -> Node {
    match reply {
        Reply::IngestDone { size } => ingested(n, size, None),
        Reply::IngestEarlyEnd(e) => ingested(n, 1, Some(e)),
        Reply::ProtectedPaths => retirement::stopped(n, Outcome::NotActionable),
        Reply::AuditAgrees => match n.early_end {
            Some(e) => retirement::stopped(n, early_outcome(e)),
            None => retirement::phased(n, Phase::Unplanned),
        },
        Reply::AuditDisagrees => {
            let rounds = if n.audit_rounds < u8::MAX { (n.audit_rounds + 1) as u8 } else { u8::MAX };
            let updated = Node { audit_rounds: rounds, ..n };
            if rounds < 2 { retirement::phased(updated, Phase::Ingesting) }
            else { asked(updated, id, QKind::AuditDisagreement,
                match n.early_end { Some(e) => Cont::Stop(early_outcome(e)), None => Cont::Unplanned }, 0) }
        },
        Reply::PlanLeaf { footprint, fits } => {
            if fits && !footprint::footprint_empty(footprint)
                && !matches!(n.phase, Phase::Planning { must_split: true }) {
                retirement::phased(Node { footprint, attempt: 1, review_restarted: false,
                    round: 0, plan_retries: 0, ..n }, Phase::Ready(Resume::Making))
            } else { plan_retry_node(n, id, fast, false) }
        },
        Reply::PlanSplit => asked(n, id, QKind::NeedsSplitting, Cont::Nowhere, 0),
        Reply::PlanCantMeet => plan_retry_node(n, id, fast, true),
        _ => n,
    }
}

/// #806: hide exact node reconstruction from unrelated production rank/bounds queries.
#[verifier::opaque]
pub(super) open spec fn node_post(source: Node, after: Node, id: u64, fast: bool, reply: Reply) -> bool {
    let expected = transitioned(source, id, fast, reply);
    after.phase == expected.phase
        && after.pos == expected.pos
        && after.repo == expected.repo
        && after.footprint == expected.footprint
        && after.generation == expected.generation
        && after.next_seq == expected.next_seq
        && after.effect == expected.effect
        && after.run == expected.run
        && after.job == expected.job
        && after.pool == expected.pool
        && after.ticket == expected.ticket
        && after.holds == expected.holds
        && after.paused == expected.paused
        && after.last_cmd == expected.last_cmd
        && after.question == expected.question
        && after.attempt == expected.attempt
        && after.round == expected.round
        && after.replanned == expected.replanned
        && after.plan_retries == expected.plan_retries
        && after.audit_rounds == expected.audit_rounds
        && after.early_end == expected.early_end
        && after.run_failures == expected.run_failures
        && after.run_incidents == expected.run_incidents
        && after.fx_failures == expected.fx_failures
        && after.fx_incidents == expected.fx_incidents
        && after.waiting_incident == expected.waiting_incident
        && after.incident_needs_rob == expected.incident_needs_rob
        && after.proposed == expected.proposed
        && after.adopted == expected.adopted
        && after.head == expected.head
        && after.checked == expected.checked
        && after.approved == expected.approved
        && after.merged == expected.merged
        && after.pr_open == expected.pr_open
        && after.had_pr == expected.had_pr
        && after.worktree == expected.worktree
        && after.rechecks == expected.rechecks
        && after.review_restarted == expected.review_restarted
        && after.credit == expected.credit
        && after.priority == expected.priority
        && after.review_stop == expected.review_stop
        && after.planning_started == expected.planning_started
        && after.waiting_prerequisite == expected.waiting_prerequisite
}

/// Q54 / #806: the exact planner frame, opaque at the production call boundary.
#[verifier::opaque]
pub(super) open spec fn plan_post(source: Node, after: Node, id: u64, fast: bool, cant: bool) -> bool {
    source.phase is Planning ==> {
        after.repo == source.repo
        && after.footprint == source.footprint
        && after.next_seq == source.next_seq
        && after.run == source.run
        && after.job == source.job
        && after.pool == source.pool
        && after.paused == source.paused
        && after.last_cmd == source.last_cmd
        && after.attempt == source.attempt
        && after.round == source.round
        && after.replanned == source.replanned
        && after.audit_rounds == source.audit_rounds
        && after.early_end == source.early_end
        && after.run_failures == source.run_failures
        && after.run_incidents == source.run_incidents
        && after.fx_failures == source.fx_failures
        && after.fx_incidents == source.fx_incidents
        && after.proposed == source.proposed
        && after.adopted == source.adopted
        && after.head == source.head
        && after.checked == source.checked
        && after.approved == source.approved
        && after.merged == source.merged
        && after.pr_open == source.pr_open
        && after.had_pr == source.had_pr
        && after.worktree == source.worktree
        && after.rechecks == source.rechecks
        && after.review_restarted == source.review_restarted
        && after.credit == source.credit
        && after.priority == source.priority
        && after.planning_started == source.planning_started
        && after.waiting_prerequisite == source.waiting_prerequisite
        && after.generation as int == source.generation as int + 1
        && after.pos == 0 && after.ticket.is_none()
        && after.effect == match source.effect {
            Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
        }
        && if fast && cant {
            after.phase == Phase::Stopping(Outcome::Stopped)
                && after.plan_retries == if source.plan_retries < u8::MAX {
                    (source.plan_retries + 1) as u8 } else { u8::MAX }
                && after.review_stop == Some(ReviewStop::PlanCantMeet)
                && after.question.is_none() && !after.waiting_incident && !after.incident_needs_rob
                && after.holds == source.holds
        } else {
            after.review_stop == source.review_stop
                && after.waiting_incident == source.waiting_incident
                && after.incident_needs_rob == source.incident_needs_rob
                && if source.plan_retries == 0 {
                    after.phase == source.phase && after.plan_retries == 1
                        && after.holds == source.holds && after.question == source.question
                } else {
                    after.phase == Phase::AwaitingRob && after.plan_retries == 0 && !after.holds
                        && after.question == Some(Question { id, kind: QKind::CantMeet,
                            cont: Cont::Planning { must_split: matches!(source.phase,
                                Phase::Planning { must_split: true }) }, cost: 1, clarified: false })
                }
        }
    }
}

/// #806 frame: only exhausted audit/planner retries and PlanSplit allocate questions.
pub(super) open spec fn asks(n: Node, fast: bool, reply: Reply) -> bool {
    match reply {
        Reply::AuditDisagrees => n.audit_rounds >= 1,
        Reply::PlanSplit => true,
        Reply::PlanCantMeet => !(fast) && n.plan_retries >= 1,
        Reply::PlanLeaf { footprint, fits } => n.plan_retries >= 1
            && (!fits || footprint::footprint_empty(footprint)
                || matches!(n.phase, Phase::Planning { must_split: true })),
        _ => false,
    }
}

/// #806: exact transition, immutable world fields, and notice-only outputs.
pub(super) open spec fn transition_post(before: World, after: World, i: int,
    source: Node, reply: Reply, out_before: Outbox, out_after: Outbox) -> bool {
    selected(reply) && before.dependencies@.len() == 0
        && port_review_reply::accepted_reply(before, source, reply) ==> {
        node_post(source, after.nodes@[i], before.next_question, before.fail_fast, reply)
            && after.next_question == (before.next_question
                + if asks(source, before.fail_fast, reply) { 1u64 } else { 0u64 }) as u64
            && after.file_sets@ == before.file_sets@
            && (retirement::notices_only(out_before) ==> retirement::notices_only(out_after))
    }
}
}
