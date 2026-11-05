// Test-only reference copied verbatim from cdacce8f99c28ca8dba8f5f76831b77b4dfcd836 crates/controller/src/adapters/workers.rs
impl Reference<'_> {
    fn build_request(
        &self,
        delivery: &Delivery,
        role: &str,
        store: &mut Store,
        snapshot: &Snapshot,
    ) -> Result<Value> {
        let node = delivery.node().0.to_string();
        let review_restarted = role == "reviewer"
            && crate::snapshot::decode(snapshot)?.nodes[delivery.node().0 as usize]
                .review_restarted;
        let review_session = if role == "reviewer" {
            review::session(delivery, store)?
        } else {
            None
        };
        let resume_review = review::resumes(delivery, review_session.as_ref(), review_restarted);
        let mut materials = match &delivery.steering {
            Some(crate::steering::Task::Classify { inputs, .. }) => inputs.clone(),
            Some(crate::steering::Task::Postmortem { id, .. }) => crate::postmortems::case_file(
                store,
                snapshot,
                delivery,
                *id,
                &self.models,
                self.limits.postmortem_deadline_secs,
            )?,
            _ => self.materials(delivery, store, snapshot)?,
        };
        if role == "planner"
            && let Some(provider) = &self.materials_provider
            && materials["prerequisite_plan_generation"] != json!(delivery.generation)
            && crate::snapshot::decode(snapshot)?
                .dependencies()
                .iter()
                .any(|dependency| dependency.node == delivery.node())
        {
            // Ingest's cached checkout predates the wait. Pin this planning
            // generation to main once, before materializing its worker checkout.
            let fresh = provider.load(delivery)?;
            materials["base_checkout"] = fresh["base_checkout"].clone();
            materials["repo"] = fresh["repo"].clone();
            materials["prerequisite_plan_generation"] = json!(delivery.generation);
            store.put_worker_materials(&node, &materials)?;
        }
        if role == "maker" && self.materials_provider.is_some() {
            let brief = store
                .worker_document(&node, "briefs")?
                .ok_or(Error::Invalid("production maker needs the accepted brief"))?;
            let plan = store
                .worker_document(&node, "plans")?
                .ok_or(Error::Invalid("production maker needs the accepted plan"))?;
            materials["maker"] = crate::production::maker_inputs(&brief, &plan)?;
            let checkout = super::worktrees::checkout_path(&self.worktrees, delivery);
            let base = if let Some(run) = &delivery.checks {
                run.base_sha.clone()
            } else {
                let worktree = Worktrees::open(&checkout, &checkout)?;
                let recorded = worktree
                    .branch_state(&self.branches.branch(delivery)?)?
                    .map(|(_, base)| base);
                let stored = materials["check_run"]["base_sha"]
                    .as_str()
                    .map(CommitId::new)
                    .transpose()?;
                match (stored, recorded) {
                    // A stored base that main has since moved past would blame main's
                    // changes on the maker (#326). The base only ever moves forward.
                    (Some(stored), Some(recorded))
                        if stored != recorded && worktree.is_ancestor(&stored, &recorded)? =>
                    {
                        recorded
                    }
                    (Some(stored), _) => stored,
                    (None, Some(recorded)) => recorded,
                    (None, None) => return Err(Error::Invalid("maker checkout has no base")),
                }
            };
            let checks = materials["maker"]["plan"]["checks"]
                .as_array()
                .ok_or(Error::Invalid("maker plan needs checks"))?
                .iter()
                .map(|c| {
                    let mut check = json!({"name":c["name"],"kind":c["kind"],"tests":c["tests"]});
                    if let Some(forcing) = c.get("forcing") {
                        check["forcing"] = forcing.clone();
                    }
                    if let Some(symptom) = c.get("declared_symptom") {
                        check["declared_symptom"] = symptom.clone();
                    }
                    if let Some(evidence) = c.get("proof") {
                        check["proof"] = evidence.clone();
                    }
                    check
                })
                .collect::<Vec<_>>();
            materials["check_run"] =
                json!({"repository":checkout,"base_sha":base.as_str(),"plan":checks});
            store.put_worker_materials(&node, &materials)?;
        }
        let mut inputs = materials.get(role).unwrap_or(&materials).clone();
        if matches!(role, "ingest" | "audit")
            && let Some(issue) = materials.get("issue")
        {
            inputs["issue"] = issue.clone();
        }
        let worktree = if role == "maker" {
            if !inputs.is_object() || !inputs["plan"].is_object() {
                return Err(Error::Invalid("maker materials need a plan object"));
            }
            if let Some(run) = &delivery.checks {
                inputs["check_base_sha"] = json!(run.base_sha);
            } else if let Some(base) = materials["check_run"].get("base_sha") {
                inputs["check_base_sha"] = base.clone();
            }
            if let Some(findings) = store.worker_document(&node, "maker_findings")? {
                let mut feedback = inputs.get("feedback").cloned().unwrap_or(json!([]));
                feedback
                    .as_array_mut()
                    .ok_or(Error::Invalid("invalid maker feedback"))?
                    .extend(
                        findings
                            .as_array()
                            .ok_or(Error::Invalid("invalid maker findings"))?
                            .iter()
                            .cloned(),
                    );
                if !feedback.as_array().unwrap().is_empty() {
                    let head = delivery.head_sha.as_deref().filter(|_| delivery.round == 1);
                    let items = feedback.as_array_mut().map(std::mem::take).unwrap();
                    inputs["feedback"] = json!(crate::findings::bounded_feedback(items, head));
                }
            }
            if let Some(conflict) = snapshot
                .rows
                .iter()
                .find(|row| row.table == "rebase_conflicts" && row.id == node)
                .map(|row| &row.value)
                && conflict["head"].as_str() == delivery.head_sha.as_deref()
            {
                let mut feedback = inputs.get("feedback").cloned().unwrap_or(json!([]));
                feedback.as_array_mut().ok_or(Error::Invalid("invalid maker feedback"))?.push(json!({
                    "kind":"failed_check", "goal_ids":[],
                    "report":crate::findings::clipped(&format!("Resolve the merge onto main {} in these files: {}. The checkout contains the pending merge, both sides in the index, and conflict markers. Preserve the request and upstream changes. Complete the resolution; worker code commits the merge. Do not abort the merge or return to the old base.", conflict["target"], conflict["files"]), crate::findings::MAX_REPORT_CHARS)
                }));
                inputs["feedback"] = feedback;
            }
            inputs["attempt"] = json!(delivery.attempt);
            inputs["round"] = json!(delivery.round);
            inputs.as_object_mut().unwrap().remove("reviewed_sha");
            let current = crate::findings::current_review(
                &inputs["feedback"],
                delivery.round,
                delivery.head_sha.as_deref(),
            );
            let reviewed = current.head;
            let has_current = !current.findings.is_empty();
            // The full text is in feedback; the maker maps these ids in done.reproductions.
            let current = json!(
                current
                    .findings
                    .iter()
                    .map(|f| json!({"id": f["id"], "reviewed_sha": f["reviewed_sha"]}))
                    .collect::<Vec<_>>()
            );
            if has_current {
                inputs["reviewed_sha"] = json!(reviewed);
            }
            inputs["current_review_findings"] = current;
            inputs["plan"]["files"] = json!(delivery.paths);
            inputs["context_size"] = json!(self.limits.context_tokens);
            inputs["tests"] = json!({"image": self.images.for_repository(&delivery.repository, &self.tests.container_image), "tools_path": self.tests.tools_dir});
            let demand = self
                .images
                .test_demand(&delivery.repository)
                .unwrap_or_default();
            inputs["tests"]["cpus"] = json!(demand.cpus);
            inputs["tests"]["mem_gb"] = json!(demand.mem_gb);
            inputs["hindsight"] =
                json!({"endpoint": self.hindsight.endpoint, "bank": self.hindsight.bank});
            super::worktrees::checkout_path(&self.worktrees, delivery)
        } else {
            self.root(&delivery.key).join("worktree")
        };
        let mut forecast = json!({"fit_tokens": self.limits.fit_tokens, "context_tokens": self.limits.context_tokens, "overhead_tokens": self.planning.overhead_tokens});
        if let Some(repository) = self.repositories.iter().find(|repo| {
            repo.owner.eq_ignore_ascii_case(&delivery.repository.owner)
                && repo.name.eq_ignore_ascii_case(&delivery.repository.name)
                && repo.installation_id == delivery.repository.installation
        }) {
            forecast["generated_files"] = json!(repository.generated_files);
            forecast["generator_timeout_s"] = json!(repository.generator_timeout_s.unwrap_or(120));
            if let Some(generator) = &repository.generator {
                forecast["generator"] = json!(generator);
            }
            forecast["generator_available"] =
                json!(repository.generator.is_some() && !repository.generated_files.is_empty());
        }
        let generator_available = forecast["generator_available"] == true;
        if role == "maker" {
            inputs["forecast"] = forecast.clone();
            inputs["protected_paths"] = json!(
                self.protected_paths
                    .for_repository(&delivery.repository, &self.planning.protected_paths)?
            );
        }
        if role == "reviewer" {
            inputs = review::inputs(
                delivery,
                store,
                snapshot,
                review_session.as_ref().filter(|_| resume_review),
            )?;
            inputs["context_size"] = json!(self.limits.context_tokens);
        } else if role == "planner" {
            let world = crate::snapshot::decode(snapshot)?;
            let n = &world.nodes[delivery.node().0 as usize];
            let ingest = store
                .worker_document(&node, "briefs")?
                .ok_or(Error::Invalid("planner needs a brief"))?;
            let findings = store
                .worker_document(&node, "findings")?
                .unwrap_or(json!([]));
            inputs = json!({"always_tests": self.images.additional_tests(&delivery.repository).always, "ingest": ingest, "must_split": matches!(n.phase, tl3_kernel::Phase::Planning { must_split: true }), "findings": findings, "protected_paths": self.protected_paths.for_repository(&delivery.repository, &self.planning.protected_paths)?, "forecast": forecast});
            inputs["repair_plan_consistency"] = json!(true);
            if let Some(base) = materials.get("base_checkout") {
                inputs["base_sha"] = base["commit"].clone();
            }
            let demand = self
                .images
                .test_demand(&delivery.repository)
                .unwrap_or_default();
            inputs["tests"] = json!({
                "image": self.images.for_repository(&delivery.repository, &self.tests.container_image),
                "tools_path": self.tests.tools_dir,
                "cpus": demand.cpus,
                "mem_gb": demand.mem_gb,
            });
        } else if role == "audit" {
            inputs["ingest"] = store
                .worker_document(&node, "briefs")?
                .ok_or(Error::Invalid("audit needs an ingest reply"))?;
        } else if role == "ingest"
            && let Some(audit) = store.worker_document(&node, "audits")?
            && audit["verdict"] == "disagree"
        {
            inputs["audit_objections"] = audit["objections"].clone();
        }
        if role == "ingest" {
            inputs["forecast"] = forecast;
        }
        if matches!(role, "ingest" | "audit" | "planner" | "maker") {
            inputs["decision_context"] =
                json!(crate::decisions::Context::load(snapshot, delivery.node())?.applicable());
        }

        if role != "reviewer" {
            inputs
                .as_object_mut()
                .ok_or(Error::Invalid("worker materials must be an object"))?
                .remove("standing_rules");
        }
        // The pristine issue and material provenance belong to the controller.
        if let Some(inputs) = inputs.as_object_mut() {
            inputs.remove("original_issue");
            inputs.remove("request_origin");
        }
        // #422: recall upfront. Ingest reads the issue, the planner its brief and
        // goals, and the reviewer the repo's conventions. An outage degrades to none.
        let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
        let goals = |goals: &Value| {
            goals
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|goal| goal["description"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let query = match role {
            "ingest" => Some(format!(
                "{}\n{}",
                inputs["issue"]["title"].as_str().unwrap_or(""),
                inputs["issue"]["body"].as_str().unwrap_or("")
            )),
            "planner" => Some(format!(
                "{}\n{}",
                inputs["ingest"]["brief"].as_str().unwrap_or(""),
                goals(&inputs["ingest"]["goals"])
            )),
            "reviewer" => Some(goals(&inputs["goals"])),
            _ => None,
        };
        if let Some(query) = &query {
            let recall = if role == "reviewer" {
                crate::memory::Recall::Conventions { repo: &repo, query }
            } else {
                crate::memory::Recall::Context { repo: &repo, query }
            };
            let recalled =
                crate::memory::recall(store, &self.hindsight, &self.recall_breaker, recall)?;
            for (field, value) in recalled.as_object().into_iter().flatten() {
                inputs[field] = value.clone();
            }
        }
        if matches!(role, "ingest" | "audit") {
            inputs["publication_branch"] = self.branches.context(delivery)?;
            inputs["protected_paths"] = json!(
                self.protected_paths
                    .for_repository(&delivery.repository, &self.planning.protected_paths)?
            );
            // Cached simulation materials may still contain inline fixture text.
            if let Some(repo) = inputs["repo"].as_object_mut() {
                repo.remove("materials");
            }
        }
        let test_cpus = self
            .images
            .test_demand(&delivery.repository)
            .unwrap_or_default()
            .cpus;
        if matches!(role, "ingest" | "audit" | "planner") {
            inputs["tool_capabilities"] =
                json!(self.planning.effective_tool_capabilities(test_cpus));
            if generator_available {
                inputs["tool_capabilities"].as_array_mut().unwrap().push(json!({
                    "tool": "run_generator", "executor": "prismabuild_generator",
                    "runner_features": ["git_index", "scoped_outputs", "idempotent_import"],
                    "artifacts": ["action_key", "cas_receipt", "input_tree", "output_tree", "artifact_sha256", "logs", "terminal_verdict"],
                    "methods": ["configured_generation"], "protected_files": [],
                }));
            }
        }
        if role == "planner" {
            inputs["audit_evidence_contract"] = store
                .worker_document(&node, "audits")?
                .and_then(|audit| audit.get("evidence_contract").cloned())
                .unwrap_or(json!([]));
        }
        protocol::validate_inputs(role, &inputs)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let resets = ladder::quota_resets(snapshot);
        let model = ladder::running(
            &self.candidates(delivery, role, store, snapshot)?,
            &resets,
            now,
        );
        let mut request = json!({
            "protocol": 1,
            "operation": {"node": delivery.node().0, "generation": delivery.generation, "kind": role},
            "role": role, "test_cpus": test_cpus, "silence_secs": self.silence_secs,
            "no_progress_secs": self.no_progress_secs,
            "model": self.models.worker_model(model), "key_file": self.models.key_file,
            "inputs": inputs, "worktree": worktree,
            "hindsight_key_file": self.hindsight.api_key_file,
            "trace": {
                "repo": format!("{}/{}", delivery.repository.owner, delivery.repository.name),
                "issue": delivery.issue, "request": node,
                "attempt": delivery.attempt, "round": delivery.round,
            },
        });
        if let Some(base) = materials["base_checkout"].get("commit") {
            request["trace"]["base_sha"] = base.clone();
        }
        if let Some(origin) = materials.get("request_origin") {
            request["trace"]["request_origin"] = origin.clone();
        }
        if role == "maker" {
            let session_file = self
                .settings
                .runs
                .join("sessions")
                .join(&node)
                .join(format!("plan-{}", u8::from(delivery.replanned)))
                .join(format!("attempt-{}.json", delivery.attempt));
            request["session_file"] = json!(session_file);
            let session = store
                .worker_document(&node, "maker_session")?
                .filter(|session| {
                    session["attempt"] == delivery.attempt
                        && session["replanned"] == delivery.replanned
                });
            if let Some(session) = &session {
                let generation = session["generation"]
                    .as_u64()
                    .ok_or(Error::Invalid("maker session has no accepted generation"))?;
                request["last_accepted_generation"] = json!(generation);
            }
            let available = session.is_some() || session_file.exists();
            let needs_files = session.is_some_and(|session| session["needs_files"] == true);
            request["resume_session"] = json!(available && (delivery.round == 1 || needs_files));
        } else if role == "planner" {
            request["session_file"] = json!(
                self.settings
                    .runs
                    .join("sessions")
                    .join(&node)
                    .join(format!("plan-{}", u8::from(delivery.replanned)))
                    .join("planner-rehearsals.json")
            );
        } else if role == "reviewer" {
            request["session_file"] = json!(
                self.settings
                    .runs
                    .join("sessions")
                    .join(&node)
                    .join(format!("plan-{}", u8::from(delivery.replanned)))
                    .join(if review_restarted {
                        format!("reviewer-attempt-{}-fresh.json", delivery.attempt)
                    } else {
                        format!("reviewer-attempt-{}.json", delivery.attempt)
                    })
            );
            // Even a retired conversation's checkpoint has an accepted generation.
            // Keep it from being mistaken for an unaccepted retry on a new head.
            if let Some(session) = &review_session {
                request["last_accepted_generation"] = json!(session["generation"].as_u64().ok_or(
                    Error::Invalid("reviewer session has no accepted generation")
                )?);
            }
            request["resume_session"] = json!(resume_review);
        }
        Ok(request)
    }

}
