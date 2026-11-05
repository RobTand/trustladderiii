use crate::reference_core as tl3_controller_core;
use super::*;
struct Shell<'a>(&'a mut Controller);
impl std::ops::Deref for Shell<'_> {type Target=Controller;fn deref(&self)->&Controller { self.0 }}
impl std::ops::DerefMut for Shell<'_> {fn deref_mut(&mut self)->&mut Controller { self.0 }}
impl Shell<'_> {
    fn translate(&mut self, snapshot: &Snapshot, world: &World, observation: Observation, commit: &mut Commit) -> Result<Option<Event>> {
        let event = match observation {
            Observation::Queue(mut fact) => {
                let identity = fact.identity();
                if fact.recorded(snapshot)? {
                    return Ok(None);
                }
                let Some(node) = fact.node(snapshot)? else {
                    return Ok(None);
                };
                use adapters::mergify::Evidence;
                use tl3_controller_core::translate::{
                    self as core, Decision, IncidentAction, KernelKind as KernelFact,
                };
                use tl3_mergify::decide::translate as queue;
                let current = &world.nodes[node.0 as usize];
                let view = queue::NodeView {
                    phase: match current.phase {
                        tl3_kernel::Phase::Publishing => queue::Phase::Publishing,
                        tl3_kernel::Phase::Queued => queue::Phase::Queued,
                        tl3_kernel::Phase::Closed(_) => queue::Phase::Closed,
                        _ => queue::Phase::Other,
                    },
                    merged: current.merged.is_some(),
                    enqueue_pending: current.effect.is_some_and(|effect| {
                        core::enqueue_pending(
                            effect.kind == tl3_kernel::EffectKind::Enqueue,
                            effect.cancelling,
                        )
                    }),
                };
                // A closed node takes only a merge it has not recorded. A later
                // active enqueue must still be able to consume head evidence.
                if !queue::admit(fact.evidence.kind(), view) {
                    return Ok(None);
                }
                if let Evidence::CiFailed {
                    head,
                    infra,
                    head_check: false,
                    details,
                    ..
                } = &mut fact.evidence
                {
                    *details = adapters::mergify::ci_evidence::bounded(details.clone());
                    let plan = check_run(snapshot, node, Some(head))?;
                    let outside = adapters::mergify::ci_evidence::outside_change(
                        &fact.repository,
                        details,
                        plan.as_ref(),
                    );
                    let previous: Vec<_> = snapshot
                        .rows
                        .iter()
                        .filter(|row| row.table == "queue_facts")
                        .map(|row| core::PriorFailure {
                            same_repository: row.value["repository"] == json!(&fact.repository),
                            same_number: row.value["number"] == fact.number,
                            same_head: row.value["evidence"]["CiFailed"]["head"] == *head,
                            infra: row.value["evidence"]["CiFailed"]["infra"] == true,
                            overlapping_test:
                                row.value["evidence"]["CiFailed"]["details"]["failed_tests"]
                                    .as_array()
                                    .is_some_and(|previous| {
                                        previous.iter().any(|previous| {
                                            details
                                                .failed_tests
                                                .iter()
                                                .any(|test| previous["test"] == test.test)
                                        })
                                    }),
                        })
                        .collect();
                    let repeated = core::repeated(&previous);
                    let (classified, decision) = queue::classify_ci(*infra, outside, repeated);
                    *infra = classified;
                    details.decision = Some(decision.as_str().into());
                }
                let value = json!(&fact);
                let head_current = match &fact.evidence {
                    Evidence::CiFailed { head, .. }
                    | Evidence::Conflict { head, .. }
                    | Evidence::DequeuedByCommand { head, .. } => core::head_current(
                        self.head(node, head)?.0,
                        world.nodes[node.0 as usize].head.map(|head| head.0),
                    ),
                    _ => true,
                };
                let approved_head = match &fact.evidence {
                    Evidence::CheckMissing { head, .. } | Evidence::QueueBlocked { head, .. } => {
                        commit_sha(snapshot, current.approved) == Some(head)
                    }
                    _ => false,
                };
                let route =
                    match queue::route(fact.evidence.kind(), view, head_current, approved_head) {
                        queue::Route::Record => core::Route::Record,
                        queue::Route::Skip => core::Route::Skip,
                        queue::Route::SetupIncident
                        | queue::Route::MissingIncident
                        | queue::Route::BlockedIncident
                        | queue::Route::ClearIncident => core::Route::Incident,
                        queue::Route::Kernel(kernel) => core::Route::Kernel(match kernel {
                            queue::KernelFact::PrMerged { checks_passed } => {
                                KernelFact::PrMerged { checks_passed }
                            }
                            queue::KernelFact::CiFailed { infra } => KernelFact::CiFailed { infra },
                            queue::KernelFact::Conflict => KernelFact::Conflict,
                            queue::KernelFact::PrDequeuedByPerson => KernelFact::PrDequeuedByPerson,
                            queue::KernelFact::PrClosedByPerson => KernelFact::PrClosedByPerson,
                            queue::KernelFact::PushByOther => KernelFact::PushByOther,
                        }),
                    };
                let kind = match &fact.evidence {
                    Evidence::ChecksUnavailable { .. } => core::EvidenceKind::Setup,
                    Evidence::CheckMissing { .. } => core::EvidenceKind::Missing,
                    Evidence::QueueBlocked { .. } => core::EvidenceKind::Blocked,
                    Evidence::CheckReported { .. } => core::EvidenceKind::Reported,
                    Evidence::Merged { .. } => core::EvidenceKind::Merged,
                    Evidence::PushByOther { .. } => core::EvidenceKind::PushByOther,
                    _ => core::EvidenceKind::Other,
                };
                let kernel = match core::dispatch(route, kind) {
                    Decision::Record => {
                        commit.rows.push(put("queue_facts", identity, value));
                        return Ok(None);
                    }
                    Decision::Skip => return Ok(None),
                    Decision::Kernel(kernel) => kernel,
                    // Setup rows (Q52, #447): none reaches the kernel, so no
                    // failure or credit is charged.
                    Decision::Incident(action) => {
                        match (action, fact.evidence) {
                            (
                                IncidentAction::Setup,
                                Evidence::ChecksUnavailable { permissions },
                            ) => {
                                self.check_setup_incident(snapshot, commit, node, &permissions)?;
                                commit.rows.push(put("queue_facts", identity, value));
                            }
                            (
                                IncidentAction::Missing,
                                Evidence::CheckMissing { head, required },
                            ) => {
                                self.missing_check_incident(
                                    snapshot, commit, node, &head, &required,
                                )?;
                                commit.rows.push(put("queue_facts", identity, value));
                            }
                            (
                                IncidentAction::Blocked,
                                Evidence::QueueBlocked {
                                    head,
                                    conditions,
                                    failed,
                                },
                            ) => {
                                self.queue_blocked_incident(
                                    snapshot,
                                    commit,
                                    node,
                                    &head,
                                    &conditions,
                                    &failed,
                                )?;
                                commit.rows.push(put("queue_facts", identity, value));
                            }
                            (IncidentAction::Clear, Evidence::CheckReported { head, required }) => {
                                let id = looks::missing_check_incident_id(node, &head, &required);
                                let incidents: Vec<_> = snapshot
                                    .rows
                                    .iter()
                                    .map(|row| core::IncidentRow {
                                        own_incident: row.table == "incidents" && row.id == id,
                                        open: row.value["open"] == true,
                                    })
                                    .collect();
                                if let Some(index) = core::clear_reported(&incidents) {
                                    let mut incident: looks::Incident =
                                        decode_stored(snapshot.rows[index].value.clone())?;
                                    incident.open = false;
                                    commit.rows.push(put("incidents", id, json!(incident)));
                                }
                            }
                            _ => {
                                return Err(Error::Invalid(
                                    "only setup evidence routes to an incident",
                                ));
                            }
                        }
                        return Ok(None);
                    }
                    Decision::InvalidIncident => {
                        return Err(Error::Invalid("only setup evidence routes to an incident"));
                    }
                    Decision::InvalidKernel => {
                        return Err(Error::Invalid(
                            "queue evidence routed to another kernel fact",
                        ));
                    }
                };
                let fact = match (kernel, fact.evidence) {
                    (KernelFact::PrMerged { checks_passed }, Evidence::Merged { head, .. }) => {
                        Fact::PrMerged {
                            head: self.head(node, &head)?,
                            checks_passed,
                        }
                    }
                    (KernelFact::CiFailed { infra }, _) => Fact::CiFailed { infra },
                    (KernelFact::Conflict, _) => Fact::Conflict,
                    (KernelFact::PrDequeuedByPerson, _) => Fact::PrDequeuedByPerson,
                    (KernelFact::PrClosedByPerson, _) => Fact::PrClosedByPerson,
                    (KernelFact::PushByOther, Evidence::PushByOther { head }) => {
                        let commits: Vec<_> = snapshot
                            .interned
                            .iter()
                            .map(|item| core::CommitRow {
                                is_commit: item.kind == tl3_store::InternKind::Commit,
                                id: item.id,
                            })
                            .collect();
                        let previous_sha =
                            core::previous_commit(&commits, current.head.map(|head| head.0 as i64))
                                .map(|index| snapshot.interned[index].value.as_str());
                        let carried = core::carry_plan(
                            node.0,
                            &head,
                            check_run(snapshot, node, previous_sha)?,
                        );
                        if let Some(carried) = carried {
                            commit.rows.push(put(
                                "check_plans",
                                format!("{}:{}", carried.node, carried.head),
                                json!(carried.plan),
                            ));
                        }
                        Fact::PushByOther {
                            head: self.head(node, &head)?,
                        }
                    }
                    _ => {
                        return Err(Error::Invalid(
                            "queue evidence routed to another kernel fact",
                        ));
                    }
                };
                commit.rows.push(put("queue_facts", identity, value));
                Event::Fact { node, fact }
            }
        };
        Ok(Some(event))
    }
}
pub(super) fn translate(controller: &mut Controller, snapshot: &Snapshot, world: &World, observation: Observation, commit: &mut Commit) -> Result<Option<Event>> {
    Shell(controller).translate(snapshot, world, observation, commit)
}
