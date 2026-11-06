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
            if n.audit_rounds == 0 { retirement::phased(updated, Phase::Ingesting) }
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
    after == transitioned(source, id, fast, reply)
}

/// Ingest/audit rows / #806: the stable fields of each accepted worker reply.
#[verifier::opaque]
pub(super) open spec fn ingest_stable(source: Node, after: Node) -> bool {
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
        && after.plan_retries == source.plan_retries
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
        && after.priority == source.priority
        && after.review_stop == source.review_stop
        && after.planning_started == source.planning_started
        && after.waiting_prerequisite == source.waiting_prerequisite

}

/// Ingest/audit rows / #806: branch-local updates, separate from the stable frame.
#[verifier::opaque]
pub(super) open spec fn ingest_changes(source: Node, after: Node, id: u64, reply: Reply) -> bool {
        let stop = reply == Reply::ProtectedPaths || reply == Reply::AuditAgrees && source.early_end.is_some();
        let ask = reply == Reply::AuditDisagrees && source.audit_rounds >= 1;
        after.generation as int == source.generation as int + 1
        && after.pos == 0 && after.ticket.is_none()
        && after.effect == match source.effect {
            Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
        }
        && after.phase == match reply {
            Reply::IngestDone { .. } | Reply::IngestEarlyEnd(_) => Phase::Auditing,
            Reply::ProtectedPaths => Phase::Stopping(Outcome::NotActionable),
            Reply::AuditAgrees => match source.early_end {
                Some(e) => Phase::Stopping(early_outcome(e)), None => Phase::Unplanned,
            },
            Reply::AuditDisagrees => if ask { Phase::AwaitingRob } else { Phase::Ingesting },
            _ => source.phase,
        }
        && after.holds == if ask { false } else { source.holds }
        && after.question == if stop { None } else if ask {
            Some(Question { id, kind: QKind::AuditDisagreement,
                cont: match source.early_end {
                    Some(e) => Cont::Stop(early_outcome(e)), None => Cont::Unplanned,
                }, cost: 0, clarified: false })
        } else { source.question }
        && after.waiting_incident == if stop { false } else { source.waiting_incident }
        && after.incident_needs_rob == if stop { false } else { source.incident_needs_rob }
        && after.audit_rounds == if reply == Reply::AuditDisagrees {
            if source.audit_rounds < u8::MAX { (source.audit_rounds + 1) as u8 } else { u8::MAX }
        } else { source.audit_rounds }
        && after.early_end == match reply {
            Reply::IngestDone { .. } => None, Reply::IngestEarlyEnd(e) => Some(e),
            _ => source.early_end,
        }
        && after.credit == match reply {
            Reply::IngestDone { size } => Credit { ready: true,
                start: if source.credit.ready { source.credit.start } else { (size as u64 + RESERVE) as u64 },
                ..source.credit },
            Reply::IngestEarlyEnd(_) => Credit { ready: true,
                start: if source.credit.ready { source.credit.start } else { (1u64 + RESERVE) as u64 },
                ..source.credit },
            _ => source.credit,
        }
}

/// Ingest/audit rows / #806: normalized fields keep the real handler query local.
#[verifier::opaque]
pub(super) open spec fn ingest_post(source: Node, after: Node, id: u64, reply: Reply) -> bool {
    ingest_accepted(source, reply) ==> ingest_stable(source, after) && ingest_changes(source, after, id, reply)
}

/// #806 planner frame: every field outside the retry, phase and question update.
#[verifier::opaque]
pub(super) open spec fn plan_stable(source: Node, after: Node) -> bool {
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
}

/// Q54 / #806: only the fields written by the real planner retry branches.
#[verifier::opaque]
pub(super) open spec fn plan_changes(source: Node, after: Node, id: u64, fast: bool, cant: bool) -> bool {
    after.generation as int == source.generation as int + 1
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

/// Q54 / #806: combine the stable frame and exact branch fields at the call boundary.
#[verifier::opaque]
pub(super) open spec fn plan_post(source: Node, after: Node, id: u64, fast: bool, cant: bool) -> bool {
    source.phase is Planning ==> plan_stable(source, after) && plan_changes(source, after, id, fast, cant)
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

/// #806 reply outputs: one Ask, one measures notice, or the unchanged outbox.
pub(super) open spec fn emission(n: Node, id: u64, fast: bool, reply: Reply) -> Option<Notice> {
    if asks(n, fast, reply) { Some(Notice::Ask { question: id }) }
    else if reply == Reply::AuditAgrees && n.early_end.is_none() { Some(Notice::MeasuresPosted) }
    else { None }
}

/// #806 output frame: the handler changes exactly one append slot or no slot.
#[verifier::opaque]
pub(super) open spec fn output_post(before: Outbox, after: Outbox, i: int, emitted: Option<Notice>) -> bool {
    before.len < OUTBOX && match emitted {
        Some(notice) => after.len == before.len + 1
            && after.items@ == before.items@.update(before.len as int,
                Some(Out::Notice { node: NodeId(i as u64), notice })),
        None => after == before,
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
            && (reply == Reply::PlanCantMeet ==> port_failfast_diversion::plan_result(
                source, after.nodes@[i], before.fail_fast, true,
                before.next_question, after.next_question, out_before, out_after))
    }
}
}
