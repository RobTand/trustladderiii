use super::*;
struct Shell<'a>(&'a mut Controller);
impl std::ops::Deref for Shell<'_> { type Target=Controller; fn deref(&self)->&Controller { self.0 } }
impl std::ops::DerefMut for Shell<'_> { fn deref_mut(&mut self)->&mut Controller { self.0 } }
impl Shell<'_> {
    fn translate(&mut self, snapshot: &Snapshot, world: &mut World, observation: Observation, commit: &mut Commit) -> Result<(Option<Event>, bool)> {
        let mut detailed_review = false;
        let event = self.arm(snapshot, world, observation, commit, &mut detailed_review)?;
        Ok((event, detailed_review))
    }
    fn arm(&mut self, snapshot: &Snapshot, world: &mut World, observation: Observation, commit: &mut Commit, detail: &mut bool) -> Result<Option<Event>> {
        let mut detailed_review = false;
        let event = match observation {
            Observation::WorkerFailed {
                node,
                generation,
                failure_class,
                failure_message,
            } => {
                use tl3_controller_core::translate_worker as worker;
                let n = world
                    .nodes
                    .get(node.0 as usize)
                    .ok_or(Error::Invalid("worker node missing"))?;
                if worker::worker_failed(n.run, n.generation, generation) == worker::Failure::Ignore
                {
                    return Ok(None);
                }
                commit.rows.push(put(
                    "worker_failures",
                    node.0.to_string(),
                    json!({
                        "node": node, "generation": generation,
                        "failure_class": failure_class, "failure_message": failure_message,
                    }),
                ));
                Event::Reply {
                    node,
                    generation,
                    reply: Reply::RunFailed,
                }
            }
            Observation::WorkerReply {
                node,
                generation,
                role,
                mut reply,
                model,
            } => {
                use tl3_controller_core::translate_worker as worker;
                let n = world
                    .nodes
                    .get(node.0 as usize)
                    .copied()
                    .ok_or(Error::Invalid("worker node missing"))?;
                if worker::worker_failed(n.run, n.generation, generation) == worker::Failure::Ignore
                {
                    return Ok(None);
                }
                let phase = match n.phase {
                    tl3_kernel::Phase::Ingesting => worker::Phase::Ingesting,
                    tl3_kernel::Phase::Auditing => worker::Phase::Auditing,
                    tl3_kernel::Phase::Planning { .. } => worker::Phase::Planning,
                    tl3_kernel::Phase::Making => worker::Phase::Making,
                    tl3_kernel::Phase::Reviewing => worker::Phase::Reviewing,
                    _ => worker::Phase::Other,
                };
                let worker_role = match role.as_str() {
                    "ingest" => worker::Role::Ingest,
                    "audit" => worker::Role::Audit,
                    "planner" => worker::Role::Planner,
                    "maker" => worker::Role::Maker,
                    "reviewer" => worker::Role::Reviewer,
                    "writer" => worker::Role::Writer,
                    "classifier" => worker::Role::Classifier,
                    "postmortem" => worker::Role::Postmortem,
                    _ => worker::Role::Unknown,
                };
                let valid_role = worker::valid_role(phase, worker_role);
                let mut mapped = match adapters::workers::protocol::map_reply(&role, &reply) {
                    Ok(reply) if valid_role => reply,
                    _ => {
                        return Ok(Some(Event::Reply {
                            node,
                            generation,
                            reply: Reply::RunFailed,
                        }));
                    }
                };
                self.clear_model_quota(snapshot, model.as_deref(), commit);
                if let Some(protected) =
                    self.protected_reply(snapshot, commit, node, &role, &reply)?
                {
                    mapped = adapters::workers::protocol::MappedReply::Kernel(protected);
                }
                if role == "planner" && reply["kind"] == "leaf" {
                    let checks = reply["checks"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .enumerate()
                        .map(|(index, check)| {
                            let declared = |field: &str| check.get(field).cloned();
                            Ok(adapters::prismabuild::Check {
                                name: format!("check-{}", index + 1),
                                kind: serde_json::from_value(check["kind"].clone())?,
                                tests: adapters::workers::protocol::reply_field(check, "node_ids")?,
                                forcing: declared("forcing")
                                    .map(serde_json::from_value)
                                    .transpose()?,
                                proof: declared("proof").map(serde_json::from_value).transpose()?,
                                declared_symptom: declared("declared_symptom")
                                    .map(serde_json::from_value)
                                    .transpose()?,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    if let Err(error) = adapters::prismabuild::validate_plan_size(&checks) {
                        match error {
                            Error::Invalid(message) => {
                                reply["violations"]
                                    .as_array_mut()
                                    .unwrap()
                                    .push(json!(message));
                                mapped = adapters::workers::protocol::MappedReply::Kernel(
                                    Reply::PlanCantMeet,
                                );
                            }
                            error => return Err(error),
                        }
                    }
                }
                if role == "reviewer" {
                    let mut criteria =
                        adapters::workers::review::criteria(&mut self.store, snapshot, node)?;
                    criteria["head_sha"] = json!(n.head.and_then(|head| {
                        snapshot
                            .interned
                            .iter()
                            .find(|i| {
                                i.kind == tl3_store::InternKind::Commit && i.id == head.0 as i64
                            })
                            .map(|i| i.value.clone())
                    }));
                    let blockers =
                        match adapters::workers::review::validate_reply(&reply, &criteria) {
                            Ok(blockers) => blockers,
                            Err(_) => {
                                return Ok(Some(Event::Reply {
                                    node,
                                    generation,
                                    reply: Reply::RunFailed,
                                }));
                            }
                        };
                    commit.rows.push(put(
                        "review_evidence",
                        format!("{}:{generation:020}", node.0),
                        evidence::review_record(
                            &json!({"head_sha":criteria["head_sha"],"verdict":reply["verdict"],
                            "blocking_findings":reply["blocking_findings"]}),
                        ),
                    ));
                    let verdict = match reply["verdict"].as_str() {
                        Some("approve") => worker::ReviewVerdict::Approve,
                        Some("changes") => worker::ReviewVerdict::Changes,
                        _ => worker::ReviewVerdict::Other,
                    };
                    let route = worker::review_route(verdict, n.head.map(|head| head.0));
                    match route {
                        worker::ReviewRoute::Approve(head) => {
                            mapped =
                                adapters::workers::protocol::MappedReply::Kernel(Reply::Approved {
                                    head: HeadId(head),
                                });
                        }
                        worker::ReviewRoute::MissingHead => {
                            return Err(Error::Invalid("review has no head"));
                        }
                        _ => {}
                    }
                    if route == worker::ReviewRoute::Changes {
                        let mut feedback = self
                            .store
                            .worker_document(&node.0.to_string(), "maker_findings")?
                            .unwrap_or(json!([]));
                        let reports = feedback
                            .as_array_mut()
                            .ok_or(Error::Invalid("invalid maker findings"))?;
                        let formatted: Vec<_> = blockers
                            .iter()
                            .take(findings::MAX_REVIEW_FINDINGS)
                            .map(|finding| {
                                format!(
                                    "{}: {}",
                                    findings::clipped(
                                        &finding.problem,
                                        findings::MAX_MESSAGE_CHARS
                                    ),
                                    findings::clipped(
                                        &finding.evidence,
                                        findings::MAX_MESSAGE_CHARS
                                    ),
                                )
                            })
                            .collect();
                        let lengths: Vec<_> = formatted
                            .iter()
                            .map(|report| report.chars().count())
                            .collect();
                        let count = worker::findings_prefix(
                            &lengths,
                            findings::MAX_REVIEW_FINDINGS,
                            findings::MAX_REPORT_CHARS,
                        );
                        for (index, (finding, report)) in
                            blockers.iter().zip(formatted).take(count).enumerate()
                        {
                            let goal_ids = if finding.citation.kind
                                == adapters::workers::review::CitationKind::Goal
                            {
                                vec![finding.citation.id.clone()]
                            } else {
                                Vec::new()
                            };
                            reports.push(serde_json::to_value(findings::Finding {
                                id: Some(format!("review-{generation}-{index}")),
                                reviewed_sha: Some(
                                    criteria["head_sha"]
                                        .as_str()
                                        .ok_or(Error::Invalid(
                                            "review finding needs its exact commit",
                                        ))?
                                        .to_owned(),
                                ),
                                kind: findings::Kind::ReviewFinding,
                                report,
                                goal_ids,
                                failed_tests: Vec::new(),
                                outside_paths: Vec::new(),
                                outside_paths_total: 0,
                                ci_checks: Vec::new(),
                            })?);
                        }
                        commit
                            .rows
                            .push(put("maker_findings", node.0.to_string(), feedback));
                        detailed_review = true;
                    }
                }
                let document = worker::document(worker_role).map(|document| match document {
                    worker::Document::Briefs => "briefs",
                    worker::Document::Audits => "audits",
                    worker::Document::MakerReplies => "maker_replies",
                    worker::Document::Plans => "plans",
                });
                if role == "planner" {
                    let mut findings = self
                        .store
                        .worker_document(&node.0.to_string(), "findings")?
                        .unwrap_or(json!([]));
                    let reports = findings
                        .as_array_mut()
                        .ok_or(Error::Invalid("findings must be text reports"))?;
                    for violation in adapters::workers::protocol::reply_field::<Vec<String>>(
                        &reply,
                        "violations",
                    )? {
                        let violation = json!(violation);
                        if !reports.contains(&violation) {
                            reports.push(violation);
                        }
                    }
                    if let Some(report) = reply["report"].as_str() {
                        reports.push(json!(report));
                    }
                    commit
                        .rows
                        .push(put("findings", node.0.to_string(), findings));
                    if let adapters::workers::protocol::MappedReply::Kernel(Reply::PlanLeaf {
                        fits,
                        ..
                    }) = mapped
                    {
                        let paths = adapters::workers::protocol::reply_field::<Vec<String>>(
                            &reply, "files",
                        )?;
                        mapped =
                            adapters::workers::protocol::MappedReply::Kernel(Reply::PlanLeaf {
                                footprint: self.footprint(snapshot, world, node, &paths)?,
                                fits,
                            });
                    }
                }
                if let Some(document) = document {
                    commit
                        .rows
                        .push(put(document, node.0.to_string(), reply.clone()));
                }
                if role == "ingest" && reply["kind"] == "brief" {
                    self.prepare_merge_dependencies(snapshot, world, node, &reply, commit)?;
                }
                if role == "ingest" {
                    commit.rows.push(put(
                        "goals",
                        node.0.to_string(),
                        json!(crate::goals::identified(
                            reply.get("goals").unwrap_or(&json!([]))
                        )?),
                    ));
                }
                let mapped = match mapped {
                    adapters::workers::protocol::MappedReply::Kernel(reply) => reply,
                    adapters::workers::protocol::MappedReply::Made(sha) => {
                        let inputs = self
                            .store
                            .worker_materials(&node.0.to_string())?
                            .ok_or(Error::Invalid("maker materials missing"))?;
                        let goals = inputs.get("maker").unwrap_or(&inputs)["goals"]
                            .as_array()
                            .ok_or(Error::Invalid("maker goals missing"))?;
                        let checks = adapters::workers::protocol::reply_field::<Vec<Value>>(
                            &reply,
                            "self_check",
                        )?;
                        let expected: BTreeSet<_> =
                            goals.iter().map(|g| g["id"].as_str()).collect();
                        let checked: BTreeSet<_> =
                            checks.iter().map(|c| c["goal_id"].as_str()).collect();
                        let feedback = self
                            .store
                            .worker_document(&node.0.to_string(), "maker_findings")?
                            .unwrap_or(json!([]));
                        let previous = commit_sha(snapshot, n.head);
                        let current = findings::current_review(&feedback, n.round, previous);
                        let reviewed = current.head;
                        let fixes = adapters::workers::reproducing::check(&reply, current);
                        let same_goals = expected == checked;
                        let same_count = checks.len() == goals.len();
                        let handover_valid = same_goals
                            && same_count
                            && crate::goals::handover(goals, &checks).is_ok();
                        if !worker::maker_self_check(same_goals, same_count, handover_valid) {
                            Reply::RunFailed
                        } else if let Ok(fixes) = fixes {
                            if !fixes.evidence["findings"].as_array().unwrap().is_empty() {
                                let mut run = check_run(snapshot, node, Some(&sha))?
                                    .or(check_run(snapshot, node, previous)?)
                                    .ok_or(Error::Invalid(
                                        "review fix needs its original check plan",
                                    ))?;
                                run.plan.retain(|check| {
                                    check.kind != adapters::prismabuild::Kind::Reproducing
                                });
                                run.plan.extend(fixes.checks);
                                run.reviewed_sha = run
                                    .plan
                                    .iter()
                                    .any(|check| {
                                        check.kind == adapters::prismabuild::Kind::Reproducing
                                    })
                                    .then_some(reviewed)
                                    .flatten()
                                    .map(tl3_worktrees::CommitId::new)
                                    .transpose()?;
                                commit.rows.push(put(
                                    "check_plans",
                                    format!("{}:{sha}", node.0),
                                    json!(run),
                                ));
                                let mut evidence = fixes.evidence;
                                evidence["head_sha"] = json!(sha);
                                commit
                                    .rows
                                    .push(put("review_fixes", node.0.to_string(), evidence));
                            }
                            Reply::MakerDone {
                                head: self.head(node, &sha)?,
                            }
                        } else {
                            Reply::RunFailed
                        }
                    }
                    adapters::workers::protocol::MappedReply::NeedsFiles { paths, fits } => {
                        let request = request(snapshot, node)?;
                        let repo = self
                            .store
                            .intern_repo(&request.repo.owner, &request.repo.name)?;
                        let mut files = world
                            .files(n.footprint)
                            .ok_or(Error::Invalid("unknown footprint"))?
                            .collect::<Vec<_>>();
                        for path in paths {
                            files.push(tl3_kernel::FileId(
                                self.store.intern_path(repo, &path)?.get() as u64,
                            ));
                        }
                        Reply::MakerNeedsFiles {
                            files: world.intern_footprint(files),
                            fits,
                        }
                    }
                };
                if mapped != Reply::RunFailed {
                    commit.rows.push(RowChange::Delete {
                        table: "worker_failures".into(),
                        id: node.0.to_string(),
                    });
                }
                if role == "reviewer" {
                    commit.rows.push(put(
                        "reviewer_session",
                        node.0.to_string(),
                        json!({"attempt": n.attempt, "replanned": n.replanned,
                            "review_restarted": n.review_restarted,
                            "out_of_room": reply["verdict"] == "out_of_room",
                            "generation": generation,
                            "head_sha": n.head.and_then(|head| snapshot.interned.iter()
                                .find(|i| i.kind == tl3_store::InternKind::Commit && i.id == head.0 as i64)
                                .map(|i| i.value.clone()))}),
                    ));
                }
                if role == "maker" && mapped != Reply::RunFailed {
                    commit.rows.push(put(
                        "maker_session",
                        node.0.to_string(),
                        json!({
                            "attempt": n.attempt, "replanned": n.replanned, "generation": generation,
                            "needs_files": reply["kind"] == "needs_files" && reply["fits"] == true,
                            "model": model,
                        }),
                    ));
                }
                Event::Reply {
                    node,
                    generation,
                    reply: mapped,
                }
            }
        };
        *detail = detailed_review;
        Ok(Some(event))
    }
}
pub(super) fn translate(c: &mut Controller, s: &Snapshot, w: &mut World, o: Observation, commit: &mut Commit) -> Result<(Option<Event>, bool)> { Shell(c).translate(s,w,o,commit) }
