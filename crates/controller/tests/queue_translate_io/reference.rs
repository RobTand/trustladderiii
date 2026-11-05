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
                use tl3_mergify::decide::translate::{self as queue, KernelFact, Route};
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
                        effect.kind == tl3_kernel::EffectKind::Enqueue && !effect.cancelling
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
                    let repeated = snapshot
                        .rows
                        .iter()
                        .filter(|row| row.table == "queue_facts")
                        .any(|row| {
                            row.value["repository"] == json!(&fact.repository)
                                && row.value["number"] == fact.number
                                && row.value["evidence"]["CiFailed"]["head"] == *head
                                && row.value["evidence"]["CiFailed"]["infra"] == true
                                && row.value["evidence"]["CiFailed"]["details"]["failed_tests"]
                                    .as_array()
                                    .is_some_and(|previous| {
                                        previous.iter().any(|previous| {
                                            details
                                                .failed_tests
                                                .iter()
                                                .any(|test| previous["test"] == test.test)
                                        })
                                    })
                        });
                    let (classified, decision) = queue::classify_ci(*infra, outside, repeated);
                    *infra = classified;
                    details.decision = Some(decision.as_str().into());
                }
                let value = json!(&fact);
                let head_current = match &fact.evidence {
                    Evidence::CiFailed { head, .. }
                    | Evidence::Conflict { head, .. }
                    | Evidence::DequeuedByCommand { head, .. } => {
                        Some(self.head(node, head)?) == world.nodes[node.0 as usize].head
                    }
                    _ => true,
                };
                let approved_head = match &fact.evidence {
                    Evidence::CheckMissing { head, .. } | Evidence::QueueBlocked { head, .. } => {
                        commit_sha(snapshot, current.approved) == Some(head)
                    }
                    _ => false,
                };
                let kernel =
                    match queue::route(fact.evidence.kind(), view, head_current, approved_head) {
                        Route::Record => {
                            commit.rows.push(put("queue_facts", identity, value));
                            return Ok(None);
                        }
                        Route::Skip => return Ok(None),
                        Route::Kernel(kernel) => kernel,
                        // Setup rows (Q52, #447): none reaches the kernel, so no
                        // failure or credit is charged.
                        Route::SetupIncident
                        | Route::MissingIncident
                        | Route::BlockedIncident
                        | Route::ClearIncident => {
                            match fact.evidence {
                                Evidence::ChecksUnavailable { permissions } => {
                                    self.check_setup_incident(
                                        snapshot,
                                        commit,
                                        node,
                                        &permissions,
                                    )?;
                                    commit.rows.push(put("queue_facts", identity, value));
                                }
                                Evidence::CheckMissing { head, required } => {
                                    self.missing_check_incident(
                                        snapshot, commit, node, &head, &required,
                                    )?;
                                    commit.rows.push(put("queue_facts", identity, value));
                                }
                                Evidence::QueueBlocked {
                                    head,
                                    conditions,
                                    failed,
                                } => {
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
                                Evidence::CheckReported { head, required } => {
                                    let id =
                                        looks::missing_check_incident_id(node, &head, &required);
                                    if let Some(row) = snapshot.rows.iter().find(|row| {
                                        row.table == "incidents"
                                            && row.id == id
                                            && row.value["open"] == true
                                    }) {
                                        let mut incident: looks::Incident =
                                            decode_stored(row.value.clone())?;
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
                        let previous_sha = current.head.and_then(|head| {
                            snapshot
                                .interned
                                .iter()
                                .find(|i| {
                                    i.kind == tl3_store::InternKind::Commit && i.id == head.0 as i64
                                })
                                .map(|i| i.value.as_str())
                        });
                        if let Some(run) = check_run(snapshot, node, previous_sha)? {
                            commit.rows.push(put(
                                "check_plans",
                                format!("{}:{head}", node.0),
                                json!(run),
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
