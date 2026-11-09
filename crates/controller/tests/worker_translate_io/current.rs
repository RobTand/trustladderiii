use super::*;
struct Shell<'a>(&'a mut Controller);
impl std::ops::Deref for Shell<'_> { type Target=Controller; fn deref(&self)->&Controller { self.0 } }
impl std::ops::DerefMut for Shell<'_> { fn deref_mut(&mut self)->&mut Controller { self.0 } }
impl Shell<'_> {
    fn footprint(
        &mut self,
        snapshot: &Snapshot,
        world: &mut World,
        node: NodeId,
        paths: &[String],
    ) -> Result<tl3_kernel::Footprint> {
        let request = request(snapshot, node)?;
        let paths = tl3_contracts::footprint::Footprint::new(paths.to_vec()).map_err(|reason| {
            let n = &world.nodes[node.0 as usize];
            Error::FootprintInvariant {
                path: paths
                    .iter()
                    .find(|path| {
                        tl3_contracts::footprint::Footprint::new(vec![(*path).clone()]).is_err()
                    })
                    .expect("Footprint rejects an invalid individual path")
                    .clone(),
                request: format!(
                    "{}/{}#{}",
                    request.repo.owner, request.repo.name, request.issue
                ),
                node: node.0,
                attempt: n.attempt,
                run: n
                    .run
                    .map_or_else(|| "<absent>".into(), |run| run.to_string()),
                reason,
            }
        })?;
        let repo = self
            .store
            .intern_repo(&request.repo.owner, &request.repo.name)?;
        let files = paths
            .iter()
            .map(|p| {
                self.store
                    .intern_path(repo, p)
                    .map_err(Error::from)
                    .map(|id| tl3_kernel::FileId(id.get() as u64))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(world.intern_footprint(files))
    }

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
                feedback.failure_message = Some(failure_message.clone());
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
                plan_check,
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
                let mut mapped = match adapters::workers::protocol::map_reply_with_plan_check(
                    &role,
                    &reply,
                    plan_check.as_ref(),
                ) {
                    Ok(reply) if valid_role => reply,
                    _ => {
                        return Ok(Some(Event::Reply {
                            node,
                            generation,
                            reply: Reply::RunFailed,
                        }));
                    }
                };
                // Legacy quota rows have no ordering identity. Preserve their
                // once-only reply consumption until fresh quota facts replace them.
                if let Some(row) = snapshot.rows.iter().find(|row| {
                    row.table == "model_quotas"
                        && Some(row.id.as_str()) == model.as_deref()
                        && row.value["finished_ms"].is_null()
                }) {
                    commit.rows.push(RowChange::Delete {
                        table: row.table.clone(),
                        id: row.id.clone(),
                    });
                }
                if let Some(protected) =
                    self.protected_reply(snapshot, commit, node, &role, &reply)?
                {
                    mapped = adapters::workers::protocol::MappedReply::Kernel(protected);
                }
                if role == "planner" && reply["kind"] == "leaf" && plan_check.is_none() {
                    let checks = reply["checks"]
                        .as_array()
                        .ok_or(Error::Invalid("planner checks must be an array"))?
                        .iter()
                        .map(|check| {
                            serde_json::from_value::<adapters::prismabuild::Check>(check.clone())
                                .map_err(Error::from)
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let brief = self
                        .store
                        .worker_document(&node.0.to_string(), "briefs")?
                        .ok_or(Error::Invalid("planner admission needs the accepted brief"))?;
                    let goals = crate::goals::identified(&brief["goals"])?;
                    let required = goals
                        .iter()
                        .enumerate()
                        .filter(|(_, goal)| goal["stage"] != "publishing")
                        .map(|(index, _)| tl3_contracts::GoalId(index as u64))
                        .collect::<Vec<_>>();
                    let admission =
                        tl3_contracts::checks::admit_plan(&checks, goals.len(), &required);
                    if let Err(message) = admission {
                        reply["violations"]
                            .as_array_mut()
                            .ok_or(Error::Invalid("planner violations must be an array"))?
                            .push(json!(message));
                        mapped =
                            adapters::workers::protocol::MappedReply::Kernel(Reply::PlanCantMeet);
                    }
                    if let Err(error) = adapters::prismabuild::validate_plan_size(&checks) {
                        match error {
                            Error::Invalid(message) => {
                                reply["violations"]
                                    .as_array_mut()
                                    .ok_or(Error::Invalid("planner violations must be an array"))?
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
                        feedback.review = Some(feedback::review_findings(
                            &blockers,
                            &criteria["head_sha"],
                            generation,
                        )?);
                    }
                }
                let document = worker::document(worker_role).map(|document| match document {
                    worker::Document::Briefs => "briefs",
                    worker::Document::Audits => "audits",
                    worker::Document::MakerReplies => "maker_replies",
                    worker::Document::Plans => "plans",
                });
                if role == "planner" {
                    feedback.planner = Some(reply.clone());
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
                let checkpoint = commit.rows.len();
                if let Some(document) = document {
                    let mut stored = reply.clone();
                    if let Some(provenance) = plan_check {
                        stored["plan_check"] = serde_json::to_value(provenance)?;
                    }
                    commit.rows.push(put(document, node.0.to_string(), stored));
                }
                if role == "ingest"
                    && reply["kind"] == "brief"
                    && let Err(error) =
                        self.prepare_merge_dependencies(snapshot, world, node, &reply, commit)
                {
                    if let Some(failure) = input_resources::of(&error)
                        && matches!(
                            failure.problem,
                            input_resources::Problem::KindMismatch { .. }
                        )
                    {
                        // Q7/Q51: invalid ingest output uses the existing bounded run-failure path.
                        // Discard partial declarations from this reply, atomically with its rejection.
                        commit.rows.truncate(checkpoint);
                        input_resources::record(commit, node, generation, failure);
                        return Ok(Some(Event::Reply {
                            node,
                            generation,
                            reply: Reply::RunFailed,
                        }));
                    }
                    return Err(error);
                }
                if role == "ingest" {
                    // Q7/#833: accepting the reply retires feedback atomically with its brief.
                    if snapshot.rows.iter().any(|row| {
                        row.table == "input_resource_contexts" && row.id == node.0.to_string()
                    }) {
                        commit.rows.push(RowChange::Delete {
                            table: "input_resource_contexts".into(),
                            id: node.0.to_string(),
                        });
                    }
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
                        let feedback = feedback::maker_history(snapshot, commit, node)?;
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
                            let mut evidence = findings::ReviewFixes::parse(fixes.evidence)?;
                            if !evidence.findings.is_empty() {
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
                                evidence.fields.insert("head_sha".into(), json!(sha));
                                commit.rows.push(put(
                                    "review_fixes",
                                    node.0.to_string(),
                                    json!(evidence),
                                ));
                            }
                            Reply::MakerDone {
                                head: self.head(node, &sha)?,
                            }
                        } else {
                            Reply::RunFailed
                        }
                    }
                    adapters::workers::protocol::MappedReply::NeedsFiles { paths, fits } => {
                        let paths = tl3_contracts::footprint::Footprint::new(paths)
                            .map_err(Error::Invalid)?;
                        let request = request(snapshot, node)?;
                        let repo = self
                            .store
                            .intern_repo(&request.repo.owner, &request.repo.name)?;
                        let mut files = world
                            .files(n.footprint)
                            .ok_or(Error::Invalid("unknown footprint"))?
                            .collect::<Vec<_>>();
                        for path in paths.iter() {
                            files.push(tl3_kernel::FileId(
                                self.store.intern_path(repo, path)?.get() as u64,
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
                if mapped == Reply::MakerCantMeet {
                    feedback.cant_meet = Some(reply.to_string());
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
// The current arm resolves `adapters::workers::reproducing` to the current
// production body (#824 moved node-id parsing to tl3-harness-core). The
// reference arm keeps the frozen pre-#824 copy in reproducing.rs, so no shared
// shim spans the changed leaf. The plan-size leaf likewise has a separate current copy (#835).
mod adapters {
    pub use super::super::adapters::results;
    pub mod prismabuild {
        use super::super::super::*;
        pub use tl3_controller::adapters::prismabuild::*;
        const MAX_REPORT_LINE_BYTES: usize = 4 * 1024 * 1024;
        include!("plan-size-current.rs");
    }
    pub mod workers {
        pub use super::super::super::adapters::workers::review;
        pub mod protocol {
            include!("protocol-current.rs");
        }
        pub mod reproducing {
            include!("reproducing-current.rs");
        }
    }
}
