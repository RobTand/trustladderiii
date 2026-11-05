use super::*;
impl Shell {
    pub fn observe(&mut self, key: &str, observation: Observation) -> Result<()> {
        let snapshot = self.store.load()?;
        if self.queue_message_reconciliation(&snapshot, key, &observation)? {
            return Ok(());
        }
        if let Observation::TraceLogsCapped { evicted } = observation {
            // #430: an operations note (#81), once while it stays open.
            let mut commit = Commit::new(
                format!("trace-logs-capped:{key}:{}", snapshot.revision.get()),
                json!({"trace_logs_capped": evicted}),
                &snapshot,
            );
            self.input_failed(
                &snapshot,
                "worker-trace-logs",
                None,
                "Held worker span logs",
                None,
                executor::Upstream::Workers,
                Error::Invalid(
                    "held span logs reached their disk cap; the oldest unshipped ones were dropped",
                ),
                &mut commit,
            )?;
            if !commit.rows.is_empty() {
                self.store.commit(&commit)?;
            }
            return Ok(());
        }
        let news = observation_upstream(&snapshot, &observation)?;
        if let Observation::Queue(fact) = &observation {
            let identity = fact.identity();
            let recorded = fact.recorded(&snapshot)?;
            let inboxed = !recorded && snapshot.rows.iter().any(|row| {
                    row.table == "inbox"
                        && serde_json::from_value::<Observation>(row.value.clone())
                            .ok()
                            .is_some_and(|pending| matches!(pending, Observation::Queue(pending) if pending.identity() == identity))
                });
            if tl3_mergify::decide::translate::intake_duplicate(recorded, inboxed) {
                return Ok(());
            }
        }
        if let Observation::GitHubComment {
            repository,
            comment,
            ..
        } = &observation
        {
            let id = format!(
                "{}/{}/{}/{}",
                repository.owner.to_ascii_lowercase(),
                repository.name.to_ascii_lowercase(),
                repository.installation,
                comment.id
            );
            if self.store.comment_retired(&id)?
                || snapshot.rows.iter().any(|row| {
                    (row.table == "github_comments" && row.id == id)
                        || (row.table == "inbox" && row.id == format!("external:{key}"))
                })
            {
                self.retry_held_effects(executor::Upstream::GitHub);
                return Ok(());
            }
        }
        // Setup gaps describe current health. A fresh read after a clear must
        // not be suppressed by the receipt for an earlier observation.
        let revision = tl3_mergify::decide::translate::intake_revision(
            matches!(&observation, Observation::Queue(fact) if fact.setup()),
            snapshot.revision.get(),
        );
        let key = match revision {
            Some(revision) => format!("{key}:{revision}"),
            None => key.to_owned(),
        };
        let mut commit = Commit::new(format!("observe:{key}"), json!(&observation), &snapshot);
        commit
            .rows
            .push(put("inbox", format!("external:{key}"), json!(&observation)));
        self.store.commit(&commit)?;
        if let Some(source) = news {
            self.retry_held_effects(source);
        }
        Ok(())
    }

    pub(crate) fn queue_observation(
        &mut self,
        snapshot: &Snapshot,
        commit: &mut Commit,
        key: &str,
        index: usize,
        observation: &Observation,
    ) -> Result<()> {
        if let Observation::Queue(fact) = observation
            && fact.setup()
        {
            let identity = fact.identity();
            let recorded = fact.recorded(snapshot)?;
            let inboxed = !recorded && snapshot.rows.iter().any(|row| {
                row.table == "inbox" && serde_json::from_value::<Observation>(row.value.clone())
                    .ok().is_some_and(|pending| matches!(pending, Observation::Queue(pending) if pending.identity() == identity))
            });
            if tl3_mergify::decide::translate::intake_duplicate(recorded, inboxed) {
                return Ok(());
            }
            let id = inbox_key(
                &format!(
                    "{key}:{}",
                    tl3_mergify::decide::translate::intake_revision(true, snapshot.revision.get())
                        .expect("setup intake has a revision")
                ),
                index,
                observation,
            )?;
            commit.rows.push(put("inbox", id, json!(observation)));
            return Ok(());
        }
        let id = inbox_key(key, index, observation)?;
        let aliases = [
            id.clone(),
            format!("{key}:{index:08}"),
            format!("external:look:{key}:{index}:{}", evidence_key(observation)?),
        ];
        let value = json!(observation);
        let inboxed = snapshot
            .rows
            .iter()
            .any(|row| row.table == "inbox" && aliases.contains(&row.id) && row.value == value);
        let recorded = !inboxed
            && self.store.event_matches(
                &aliases
                    .iter()
                    .map(|id| format!("consume:{id}"))
                    .collect::<Vec<_>>(),
                &value,
            )?;
        if tl3_mergify::decide::translate::intake_duplicate(recorded, inboxed) {
            return Ok(());
        }
        commit.rows.push(put("inbox", id, value));
        Ok(())
    }

    pub(super) fn translate(
        &mut self,
        snapshot: &Snapshot,
        world: &mut World,
        observation: Observation,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
        let mut ci_details = match &observation {
            Observation::CiFailed { checks, .. } => findings::QueueCiEvidence {
                checks: checks.clone(),
                ..Default::default()
            },
            Observation::Queue(adapters::mergify::QueueFact {
                evidence: adapters::mergify::Evidence::CiFailed { details, .. },
                ..
            }) => details.clone(),
            _ => Default::default(),
        };
        ci_details = adapters::mergify::ci_evidence::bounded(ci_details);
        let ci_checks = ci_details.checks.clone();
        let mut failed_tests = ci_details.failed_tests.clone();
        let mut outside_paths = Vec::new();
        let mut outside_paths_total = 0;
        let mut detailed_review = false;
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
                let route = queue::route(fact.evidence.kind(), view, head_current, approved_head);
                let kernel = match route {
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
                                self.check_setup_incident(snapshot, commit, node, &permissions)?;
                            }
                            Evidence::CheckMissing { head, required } => {
                                self.missing_check_incident(
                                    snapshot, commit, node, &head, &required,
                                )?;
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
                            }
                            Evidence::CheckReported { head, required } => {
                                let id = looks::missing_check_incident_id(node, &head, &required);
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
                        if queue::record_incident(route) {
                            commit.rows.push(put("queue_facts", identity, value));
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
            _ => return Ok(None),
        };
        let finding = match event {
            Event::Reply {
                node,
                generation,
                reply,
            } if world.nodes.get(node.0 as usize).is_some_and(|n| {
                (n.run == Some(generation) || n.job == Some(generation))
                    && n.generation == generation
            }) =>
            {
                let n = &world.nodes[node.0 as usize];
                let report = match (n.phase, reply) {
                    (tl3_kernel::Phase::Checking, Reply::ChecksDone { report: true, pass, in_footprint })
                        if !pass || !in_footprint => Some((findings::Kind::FailedCheck,
                            findings::check_report(pass, in_footprint, &failed_tests, &outside_paths, outside_paths_total))),
                    (tl3_kernel::Phase::Reviewing, Reply::ChangesRequested) if !detailed_review => Some((
                        findings::Kind::ReviewFinding,
                        "The independent reviewer requested changes. No detailed review report was supplied.".into(),
                    )),
                    (tl3_kernel::Phase::Making, Reply::MakerCantMeet) => Some((
                        findings::Kind::ReviewFinding,
                        commit.rows.iter().find_map(|row| match row {
                            RowChange::Put { table, value, .. } if table == "maker_replies" => Some(value.to_string()),
                            _ => None,
                        }).unwrap_or_else(|| "The maker could not meet the goals.".into()),
                    )),
                    _ => None,
                };
                report.map(|(kind, report)| (node, kind, report))
            }
            Event::Fact {
                node,
                fact: Fact::CiFailed { infra: false },
            } if world
                .nodes
                .get(node.0 as usize)
                .is_some_and(|n| n.phase == tl3_kernel::Phase::Queued) =>
            {
                Some((
                    node,
                    findings::Kind::FailedCheck,
                    findings::ci_report(&ci_checks, &failed_tests),
                ))
            }
            _ => None,
        };
        if let Some((node, kind, report)) = finding {
            let materials = self.store.worker_materials(&node.0.to_string())?;
            let goals = materials
                .as_ref()
                .and_then(|materials| materials.get("maker").unwrap_or(materials).get("goals"));
            let goal_ids = match goals {
                Some(goals) => goals
                    .as_array()
                    .ok_or(Error::Invalid("maker goals must be an array"))?
                    .iter()
                    .map(|goal| {
                        goal["id"]
                            .as_str()
                            .filter(|id| !id.trim().is_empty())
                            .map(str::to_owned)
                            .ok_or(Error::Invalid("maker goal needs a string id"))
                    })
                    .collect::<Result<Vec<_>>>()?,
                None => {
                    eprintln!(
                        "maker finding for node {} has no goal materials; retaining unbound evidence",
                        node.0
                    );
                    Vec::new()
                }
            };
            let mut findings = self
                .store
                .worker_document(&node.0.to_string(), "maker_findings")?
                .unwrap_or(json!([]));
            let finding = findings::Finding {
                id: None,
                reviewed_sha: None,
                kind,
                report: findings::clipped(&report, findings::MAX_REPORT_CHARS),
                goal_ids,
                failed_tests,
                outside_paths,
                outside_paths_total,
                ci_checks,
            };
            findings
                .as_array_mut()
                .ok_or(Error::Invalid("invalid maker findings"))?
                .push(serde_json::to_value(finding)?);
            commit
                .rows
                .push(put("maker_findings", node.0.to_string(), findings));
        }
        Ok(Some(self.priority_reserve(snapshot, event)?))
    }

}
