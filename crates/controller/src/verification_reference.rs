// Test-only reference copied from rust-fv 3704f839c2. Only the two method names
// and the call between them differ. Keep until #783 integrates.
use crate::*;
impl Controller {
    fn apply_reference(
        &mut self,
        snapshot: &Snapshot,
        mut world: World,
        event: Event,
        commit: &mut Commit,
    ) -> Result<()> {
        // A dequeue receipt can precede queue admission, where the kernel
        // intentionally ignores it. Only an accepted Resume releases that hold.
        if let Event::Command {
            node,
            id,
            command: tl3_kernel::Command::Resume,
        } = event
            && let Some(current) = world.nodes.get(node.0 as usize)
            && id > current.last_cmd
            && !current.phase.is_closed()
        {
            commit.rows.push(put(
                "queue_resumes",
                node.0.to_string(),
                json!({"command": id}),
            ));
        }
        let closing_cause = match event {
            Event::Command {
                node,
                command:
                    tl3_kernel::Command::Stop
                    | tl3_kernel::Command::Answer {
                        answer: tl3_kernel::Answer::Stop,
                        ..
                    },
                ..
            } => Some((node, "user_stop")),
            Event::Command {
                node,
                command:
                    tl3_kernel::Command::ChangeRequest
                    | tl3_kernel::Command::Answer {
                        answer: tl3_kernel::Answer::ChangeRequest,
                        ..
                    },
                ..
            } => Some((node, "replaced")),
            Event::Fact {
                node,
                fact: tl3_kernel::Fact::PrClosedByPerson,
            } => Some((node, "pr_closed")),
            Event::Fact {
                node,
                fact: tl3_kernel::Fact::PrDequeuedByPerson,
            } => Some((node, "pr_dequeued")),
            _ => None,
        }
        .filter(|(node, _)| {
            world.nodes.get(node.0 as usize).is_some_and(|n| {
                !matches!(
                    n.phase,
                    tl3_kernel::Phase::Stopping(_) | tl3_kernel::Phase::Closed(_)
                )
            })
        });
        let mut dependency_outputs = Vec::new();
        self.declare_merge_dependencies(&mut world, event, &mut dependency_outputs, commit)?;
        let dependency_roots = merge_dependencies::changed_roots(&world, event);
        let (mut world, mut outputs) = tl3_kernel::step(world, event);
        outputs.splice(0..0, dependency_outputs);
        self.announce_dependency_endings(
            snapshot,
            &mut world,
            dependency_roots,
            &mut outputs,
            commit,
        )?;
        if let Some((node, reason)) = closing_cause
            && world.nodes.get(node.0 as usize).is_some_and(|n| {
                matches!(
                    n.phase,
                    tl3_kernel::Phase::Stopping(_) | tl3_kernel::Phase::Closed(_)
                )
            })
        {
            commit.rows.push(put(
                "close_reasons",
                node.0.to_string(),
                json!({"reason":reason}),
            ));
        }
        self.record_kernel_result_reference(snapshot, world, outputs, commit)?;
        if !commit.result.is_object() {
            commit.result = json!({});
        }
        commit.result["kernel_event"] = json!(event);
        Ok(())
    }

    fn record_kernel_result_reference(
        &mut self,
        snapshot: &Snapshot,
        world: World,
        outputs: Vec<Out>,
        commit: &mut Commit,
    ) -> Result<()> {
        let previous = snapshot::decode(snapshot)?;
        let before = snapshot::encode(&previous)?;
        for (index, node) in world.nodes.iter().enumerate() {
            if !matches!(node.phase, tl3_kernel::Phase::Planning { .. })
                || previous
                    .nodes
                    .get(index)
                    .is_some_and(|node| matches!(node.phase, tl3_kernel::Phase::Planning { .. }))
            {
                continue;
            }
            let id = index.to_string();
            let maker_findings = commit
                .rows
                .iter()
                .rev()
                .find_map(|row| match row {
                    RowChange::Put {
                        table,
                        id: row_id,
                        value,
                    } if table == "maker_findings" && row_id == &id => Some(value),
                    _ => None,
                })
                .or_else(|| {
                    snapshot
                        .rows
                        .iter()
                        .find(|row| row.table == "maker_findings" && row.id == id)
                        .map(|row| &row.value)
                });
            if let Some(maker_findings) = maker_findings {
                let mut findings = self
                    .store
                    .worker_document(&id, "findings")?
                    .unwrap_or(json!([]));
                let reports = findings
                    .as_array_mut()
                    .ok_or(Error::Invalid("findings must be text reports"))?;
                for finding in maker_findings
                    .as_array()
                    .ok_or(Error::Invalid("invalid maker findings"))?
                {
                    if !reports.contains(&finding["report"]) {
                        reports.push(finding["report"].clone());
                    }
                }
                commit.rows.push(put("findings", id, findings));
            }
        }
        commit
            .rows
            .extend(snapshot::changes(&before, snapshot::encode(&world)?));
        self.close_replacement(snapshot, &previous, &world, commit)?;
        self.record_review_stops(snapshot, &previous, &world, commit)?;
        self.resolve_closed_effect_incidents(snapshot, &world, commit)?;
        self.record_postmortems(snapshot, &previous, &world, commit)?;
        self.resolve_stale_missing_checks(snapshot, &world, commit)?;
        for (index, node) in world.nodes.iter().enumerate() {
            if previous
                .nodes
                .get(index)
                .is_some_and(|old| old.waiting_incident)
                && !node.waiting_incident
            {
                for row in snapshot.rows.iter().filter(|r| {
                    r.table == "incidents"
                        && r.value["node"] == json!(NodeId(index as u64))
                        && r.value["open"] == true
                        && r.value["blocking"] == true
                }) {
                    let mut value = row.value.clone();
                    value["open"] = json!(false);
                    commit.rows.push(put("incidents", row.id.clone(), value));
                }
            }
        }
        let dictionary = self.store.load()?.interned;
        for (index, output) in outputs.into_iter().enumerate() {
            if let Out::Notice {
                node,
                notice: tl3_kernel::Notice::FileReplacement,
            } = output
            {
                self.pending_replacement(snapshot, &world, commit, node)?;
                continue;
            }
            let (node, key) = match output {
                Out::Effect { node, seq, .. } => (node, format!("effect:{}:{seq}", node.0)),
                Out::Notice { node, .. } => (
                    node,
                    format!("notice:{}:{index}", snapshot.revision.get() + 1),
                ),
            };
            let n = &world.nodes[node.index().ok_or(Error::Invalid("node index overflow"))?];
            let head_sha = n
                .head
                .map(|head| {
                    dictionary
                        .iter()
                        .find(|i| i.kind == tl3_store::InternKind::Commit && i.id == head.0 as i64)
                        .map(|i| i.value.clone())
                        .ok_or(Error::Invalid("head missing from intern dictionary"))
                })
                .transpose()?;
            let binding = commit.rows.iter().find_map(|row| match row {
                RowChange::Put { table, id, value }
                    if table == "requests" && id == &node.0.to_string() =>
                {
                    Some(value.clone())
                }
                _ => None,
            });
            let request: Request = match binding {
                Some(value) => decode_stored(value)?,
                None => request(snapshot, node)?,
            };
            let pull_request = snapshot
                .rows
                .iter()
                .find(|r| r.table == "pull_requests" && r.id == node.0.to_string())
                .and_then(|r| r.value["number"].as_u64());
            let paths = world
                .files(n.footprint)
                .ok_or(Error::Invalid("unknown footprint"))?
                .map(|file| {
                    dictionary
                        .iter()
                        .find(|i| i.kind == tl3_store::InternKind::Path && i.id == file.0 as i64)
                        .map(|i| i.value.clone())
                        .ok_or(Error::Invalid("path missing from intern dictionary"))
                })
                .collect::<Result<Vec<_>>>()?;
            let mut checks = check_run(snapshot, node, head_sha.as_deref())?;
            if checks.is_none()
                && matches!(
                    output,
                    Out::Effect {
                        kind: tl3_kernel::EffectKind::SubmitChecks,
                        ..
                    }
                )
                && let Some(sha) = &head_sha
                && let Some(materials) = self.store.worker_materials(&node.0.to_string())?
                && let Some(run) = materials.get("check_run")
            {
                let run: adapters::prismabuild::CheckRun = decode_stored(run.clone())?;
                commit
                    .rows
                    .push(put("check_plans", format!("{}:{sha}", node.0), json!(&run)));
                checks = Some(run);
            }
            let question_context = if let Out::Notice {
                notice: tl3_kernel::Notice::Ask { question },
                ..
            } = output
            {
                let plan = commit
                    .rows
                    .iter()
                    .rev()
                    .find_map(|row| match row {
                        RowChange::Put { table, id, value }
                            if table == "plans" && id == &node.0.to_string() =>
                        {
                            Some(value.clone())
                        }
                        _ => None,
                    })
                    .or_else(|| {
                        snapshot
                            .rows
                            .iter()
                            .find(|r| r.table == "plans" && r.id == node.0.to_string())
                            .map(|r| r.value.clone())
                    });
                let mut context = json!({"node": node, "question": question, "kind": n.question.map(|q| q.kind), "generation": n.generation, "pieces": plan.as_ref().and_then(|p| p.get("pieces")).cloned().unwrap_or(json!([])), "plan": plan});
                // #403: every question to Rob owes a postmortem.
                commit.postmortems.push(postmortems::owed(
                    snapshot,
                    node,
                    format!("question:{question}"),
                    postmortems::Reason::Escalation,
                    None,
                )?);
                // #322: Rob sees the deciding worker's findings, not only the question kind.
                let written = |table: &str| {
                    commit.rows.iter().rev().find_map(|row| match row {
                        RowChange::Put {
                            table: t,
                            id,
                            value,
                        } if t == table && id == &node.0.to_string() => Some(value.clone()),
                        _ => None,
                    })
                };
                let before = |table: &str| {
                    snapshot
                        .rows
                        .iter()
                        .find(|r| r.table == table && r.id == node.0.to_string())
                        .map(|r| r.value.clone())
                };
                let incidents: Vec<_> = commit
                    .rows
                    .iter()
                    .filter_map(|row| match row {
                        RowChange::Put { table, value, .. }
                            if table == "incidents" && value["node"] == json!(node) =>
                        {
                            Some(value.clone())
                        }
                        _ => None,
                    })
                    .collect();
                let prior = snapshot::decode(snapshot)?
                    .nodes
                    .get(node.0 as usize)
                    .map_or(tl3_kernel::Phase::Ingesting, |previous| previous.phase);
                if let Some(binding) = n.question.as_ref().filter(|binding| binding.id == question)
                    && let Some(findings) = question_findings::findings(
                        binding,
                        &question_findings::Sources {
                            fresh: &written,
                            previous: &before,
                            incidents: &incidents,
                            prior,
                        },
                    )
                {
                    context["findings"] = findings;
                }
                if let Some(failure) = worker_inputs::current(snapshot, commit, node)? {
                    context["worker_input_failure"] = json!(failure);
                }
                commit.rows.push(put(
                    "question_context",
                    question.to_string(),
                    context.clone(),
                ));
                Some(context)
            } else {
                None
            };
            let mut delivery = Delivery {
                key: key.clone(),
                epoch: snapshot.epoch.get(),
                output: output.into(),
                generation: n.generation,
                head_sha,
                repository: request.repo,
                issue: request.issue,
                pull_request,
                proposed: request.proposed,
                paths,
                attempt: n.attempt,
                round: n.round,
                replanned: n.replanned,
                checks,
                job_generation: n.job,
                question: n.question,
                marker_key: None,
                note: match output {
                    Out::Notice { notice, .. } => merge_dependencies::dependency_note(notice),
                    _ => None,
                },
                scope_removals: None,
                question_context,
                steering: None,
                request_text: request.source,
                evidence: None,
            };
            if matches!(
                output,
                Out::Notice {
                    notice: tl3_kernel::Notice::Summary(_),
                    ..
                } | Out::Effect {
                    kind: tl3_kernel::EffectKind::OpenPr | tl3_kernel::EffectKind::Enqueue,
                    ..
                }
            ) {
                delivery.evidence = Some(evidence::capture(snapshot, commit, &delivery)?);
            }
            if matches!(
                output,
                Out::Notice {
                    notice: tl3_kernel::Notice::Summary(_),
                    ..
                }
            ) {
                delivery.scope_removals = scope::summary(snapshot, node)?;
                delivery.question_context = review_stops::context(snapshot, commit, node);
                let references = &delivery
                    .evidence
                    .as_ref()
                    .expect("captured summary evidence")
                    .scratch;
                if !references.is_empty() {
                    delivery.note = Some(format!(
                        "Saved unfinished work, kept for 30 days:\n{}",
                        references.join("\n")
                    ));
                }
            }
            if matches!(
                output,
                Out::Notice {
                    notice: tl3_kernel::Notice::Summary(tl3_kernel::Outcome::NotActionable),
                    ..
                }
            ) {
                protected_requests::closing_summary(self, snapshot, commit, &mut delivery)?;
            }
            if matches!(output, Out::Notice { .. })
                && let Some(failure) = worker_inputs::current(snapshot, commit, node)?
            {
                delivery.question_context.get_or_insert_with(|| json!({}))["worker_input_failure"] =
                    json!(failure);
            }
            if let Out::Notice {
                notice: tl3_kernel::Notice::Incident { needs_rob },
                ..
            } = output
            {
                let id = looks::incident_id(snapshot, index);
                let looks::AttachedIncident {
                    upstream: mut source,
                    mut class,
                    diagnostic,
                    input_failure,
                } = self.attach_incident(snapshot, node, &id, commit)?;
                let blocked = blocked::evidence(commit, node)?;
                if blocked.is_some() {
                    source = executor::Upstream::Workers;
                    class = blocked::CLASS.into();
                }
                let signature = looks::IncidentSignature {
                    upstream: source,
                    class: class.clone(),
                };
                let needs_rob =
                    looks::incident_needs_rob(looks::IncidentCause::Kernel { needs_rob });
                let app_login = self
                    .adapters
                    .get(source)?
                    .app_login(delivery.repository.installation);
                let mut summary = looks::incident_summary(
                    &request.title,
                    &signature,
                    needs_rob,
                    app_login.as_deref(),
                );
                worker_inputs::append(&mut summary, input_failure.as_ref());
                let occurrences = 1 + snapshot
                    .rows
                    .iter()
                    .filter(|row| {
                        row.table == "incidents"
                            && row.value["node"] == json!(node)
                            && row.value["signature"] == json!(signature)
                    })
                    .count();
                if let Some(diagnostic) = &diagnostic {
                    summary.push_str(&format!(
                        " Recorded occurrence {occurrences}. {}",
                        diagnostic.render()
                    ));
                }
                if let Some(detail) = looks::queue_command_detail(&delivery, &class) {
                    summary.push(' ');
                    summary.push_str(&detail);
                }
                if class == "queue_unfinished"
                    && let Some(detail) = looks::queue_incident_detail(snapshot, node)?
                {
                    summary.push(' ');
                    summary.push_str(&detail);
                }
                if let Some(evidence) = blocked {
                    summary.push(' ');
                    summary.push_str(&evidence);
                }
                if source == executor::Upstream::Workers {
                    let failure = commit
                        .rows
                        .iter()
                        .rev()
                        .find_map(|row| match row {
                            RowChange::Put { table, id, value }
                                if table == "worker_failures" && id == &node.0.to_string() =>
                            {
                                Some(value)
                            }
                            _ => None,
                        })
                        .or_else(|| {
                            snapshot
                                .rows
                                .iter()
                                .find(|row| {
                                    row.table == "worker_failures" && row.id == node.0.to_string()
                                })
                                .map(|row| &row.value)
                        });
                    let incident_generation = snapshot
                        .rows
                        .iter()
                        .find(|row| row.table == "nodes" && row.id == node.0.to_string())
                        .and_then(|row| row.value["generation"].as_u64());
                    if let Some(failure) = failure
                        && failure["generation"].as_u64() == incident_generation
                    {
                        summary.push_str(&format!(
                            " Last worker failure: {}: {}.",
                            failure["failure_class"].as_str().unwrap_or("unknown"),
                            failure["failure_message"]
                                .as_str()
                                .unwrap_or("worker run failed")
                        ));
                    }
                }
                if needs_rob {
                    // #403: an incident that needs Rob owes a postmortem.
                    commit.postmortems.push(postmortems::owed(
                        snapshot,
                        node,
                        format!("incident:{id}"),
                        postmortems::Reason::Escalation,
                        None,
                    )?);
                }
                commit.rows.push(put(
                    "incidents",
                    id.clone(),
                    json!(looks::Incident {
                        id: id.clone(),
                        node: Some(node),
                        input_failure: None,
                        open: true,
                        needs_rob,
                        blocking: n.waiting_incident,
                        summary: summary.clone(),
                        signature: signature.clone(),
                        effect: None,
                        post_key: Some(key.clone()),
                        cause: Some(looks::Cause::of(source, &class)),
                    }),
                ));
                delivery.note = Some(looks::incident_note(
                    &delivery.repository,
                    delivery.issue,
                    &summary,
                ));
                delivery.question_context = Some(json!({"issue_title":request.title,
                    "request":format!("{} (https://github.com/{}/{}/issues/{})", request.title,
                        delivery.repository.owner, delivery.repository.name, delivery.issue),
                    "signature":signature, "incident_id":id, "publication_diagnostic":diagnostic, "occurrences":occurrences,"worker_input_failure":input_failure}));
                if let Some(operations) = &self.bootstrap.operations {
                    delivery.repository = Repository {
                        owner: operations.owner.clone(),
                        name: operations.name.clone(),
                        installation: operations.installation_id,
                    };
                    delivery.issue = operations.number;
                }
            }
            if matches!(
                output,
                Out::Effect {
                    kind: tl3_kernel::EffectKind::StartRun(_)
                        | tl3_kernel::EffectKind::StartRebase
                        | tl3_kernel::EffectKind::SubmitChecks
                        | tl3_kernel::EffectKind::Enqueue,
                    ..
                }
            ) {
                commit
                    .rows
                    .push(put("watches", key.clone(), json!(delivery)));
            }
            commit.rows.push(self.allowance_row(delivery.clone()));
            commit.effects.push(delivery.into_effect());
        }
        for (index, node) in world.nodes.iter().enumerate() {
            let old = previous
                .nodes
                .get(index)
                .map(|n| n.credit)
                .unwrap_or_default();
            let delta = node.credit.balance() - old.balance();
            if delta != 0 {
                commit.credit.push(CreditEntry {
                    node: index.to_string(),
                    root: index.to_string(),
                    reason: commit.event_key.clone(),
                    amount: delta,
                });
            }
        }
        let closed: BTreeSet<_> = world
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| matches!(node.phase, tl3_kernel::Phase::Closed(_)))
            .map(|(index, _)| NodeId(index as u64))
            .collect();
        let closed_fact = |value: &Value| -> Result<bool> {
            let fact: adapters::mergify::QueueFact = decode_stored(value.clone())?;
            Ok(fact
                .node(snapshot)?
                .is_some_and(|node| closed.contains(&node)))
        };
        let mut rows = Vec::new();
        for row in std::mem::take(&mut commit.rows) {
            if let RowChange::Put { table, value, .. } = &row
                && table == "queue_facts"
                && closed_fact(value)?
            {
                continue;
            }
            rows.push(row);
        }
        commit.rows = rows;
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "queue_facts")
        {
            if closed_fact(&row.value)? {
                commit.rows.push(RowChange::Delete {
                    table: "queue_facts".into(),
                    id: row.id.clone(),
                });
            }
        }
        for row in &snapshot.rows {
            let discard_rebase_context = row.table == "rebase_conflicts"
                && row
                    .id
                    .parse::<usize>()
                    .ok()
                    .and_then(|id| world.nodes.get(id))
                    .is_some_and(|node| {
                        node.merged.is_some() || matches!(node.phase, tl3_kernel::Phase::Closed(_))
                    });
            if discard_rebase_context
                || row.table == "check_reports"
                    && row
                        .id
                        .split(':')
                        .next()
                        .and_then(|id| id.parse::<u64>().ok())
                        .is_some_and(|id| closed.contains(&NodeId(id)))
            {
                commit.rows.push(RowChange::Delete {
                    table: row.table.clone(),
                    id: row.id.clone(),
                });
            }
        }
        self.retire_ended_effect_looks(snapshot, &world, commit)?;
        Ok(())
    }
}
