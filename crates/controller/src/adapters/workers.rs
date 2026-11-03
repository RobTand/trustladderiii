use crate::executor::Output as Out;
use crate::sync::{Arc, Mutex};
use tl3_worktrees::{CommitId, Worktrees};
pub mod writer;
pub use writer::{FakeWriter, Writer};

mod evidence;
mod facts;
mod fake;
#[cfg(test)]
mod input_contract_tests;
mod json;
pub(crate) mod ladder;
#[cfg(test)]
mod postmortem_failure_tests;
mod process;
pub(crate) mod protocol;
pub(crate) mod reproducing;
#[cfg(test)]
mod restart_base_tests;
pub(crate) mod review;
mod source;
mod source_error;
#[cfg(test)]
mod startup_tests;
mod trace;
#[cfg(test)]
mod trace_tests;
pub use fake::Fake;
pub use source::WorkerOutputSource;
pub use source_error::SourceError;

use crate::{
    Error, Result,
    executor::{CommentKind, Completion, Delivery, Executor, Look, Observation, Reconciliation},
};
use process::{alive, cancel, read, write};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};
use tl3_kernel::{EffectKind, Event, Reply};
use tl3_store::{Snapshot, Store};

pub(super) fn isolate_credentials(
    attempt: &Path,
    request: &mut Value,
    command: &mut Command,
) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let source = Path::new(
        request["key_file"]
            .as_str()
            .ok_or(Error::Invalid("worker needs key_file"))?,
    );
    let directory = attempt.join("credentials");
    fs::DirBuilder::new().mode(0o700).create(&directory)?;
    let key = directory.join("model-key");
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&key)?;
    std::io::copy(&mut File::open(source)?, &mut output)?;
    output.sync_all()?;
    File::open(&directory)?.sync_all()?;
    request["key_file"] = json!(key);
    command.env("CREDENTIALS_DIRECTORY", &directory);
    Ok(())
}

#[derive(Clone, Default)]
pub(super) struct ActiveRuns(Arc<Mutex<BTreeMap<String, BTreeMap<String, i64>>>>);
/// Jobs outside the kernel are keyed apart, so stopping a node's run never
/// stops its comment classification or postmortem.
fn active_key(delivery: &Delivery) -> String {
    if delivery.is_classifier() {
        format!("comment:{}", delivery.node().0)
    } else if delivery.is_postmortem() {
        format!("postmortem:{}", delivery.node().0)
    } else {
        delivery.node().0.to_string()
    }
}

impl ActiveRuns {
    fn add(&self, delivery: &Delivery, attempt: i64) {
        self.0
            .lock()
            .unwrap()
            .entry(active_key(delivery))
            .or_default()
            .entry(delivery.key.clone())
            .and_modify(|current| *current = (*current).max(attempt))
            .or_insert(attempt);
    }
    fn remove(&self, delivery: &Delivery, attempt: i64) {
        let mut nodes = self.0.lock().unwrap();
        let node = active_key(delivery);
        if let Some(keys) = nodes.get_mut(&node) {
            if keys.get(&delivery.key) == Some(&attempt) {
                keys.remove(&delivery.key);
            }
            if keys.is_empty() {
                nodes.remove(&node);
            }
        }
    }
    fn remove_key(&self, key: &str) {
        self.0.lock().unwrap().retain(|_, keys| {
            keys.remove(key);
            !keys.is_empty()
        });
    }
    fn keys(&self, node: &str) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .get(node)
            .map(|keys| keys.keys().cloned().collect())
            .unwrap_or_default()
    }
}

#[derive(Clone)]
pub struct Supervisor {
    branches: crate::branches::Branches,
    settings: tl3_config::Workers,
    models: tl3_config::Models,
    planning: tl3_config::Planning,
    protected_paths: crate::scope::ProtectedPaths,
    repositories: Vec<tl3_config::Repository>,
    limits: tl3_config::Limits,
    // The worker bounds the maker's retro by these allowances (#648).
    silence_secs: u32,
    no_progress_secs: u32,
    store: tl3_config::Store,
    records_root: PathBuf,
    materials_provider: Option<crate::production::Materials>,
    active: ActiveRuns,
    worktrees: PathBuf,
    tests: tl3_config::PrismaBuild,
    images: super::images::Images,
    hindsight: tl3_config::Hindsight,
    memory: crate::memory::Feed,
    recall_breaker: crate::memory::Breaker,
    announcer: crate::memory::Announcer,
}

impl Supervisor {
    pub fn new(config: &tl3_config::Config, records_root: &Path) -> Result<Self> {
        protocol::initialize_schemas()?;
        fs::create_dir_all(config.workers.runs.join(".ready"))?;
        fs::create_dir_all(records_root)?;
        let active = ActiveRuns::default();
        let mut store = Store::open(&config.store)?;
        crate::prompts::seed(&mut store)?;
        for delivery in watched_deliveries(&mut store)? {
            let root = run_root(&config.workers.runs, &delivery.key);
            match latest(&root).and_then(|attempt| match attempt {
                Some((_, path)) => inspect_recorded(&path, &delivery, &mut store),
                None => Ok(Reconciliation::Absent),
            }) {
                Ok(Reconciliation::Pending) => {
                    active.add(&delivery, latest(&root)?.expect("pending attempt").0)
                }
                Ok(_) => {}
                Err(error) => eprintln!("worker startup skipped {}: {error}", delivery.key),
            }
        }
        let recall_breaker = crate::memory::Breaker::default();
        let announcer = crate::memory::Announcer::default();
        Ok(Self {
            branches: crate::branches::Branches::new(&config.github.repos)?,
            active,
            settings: config.workers.clone(),
            models: config.models.clone(),
            planning: config.planning.clone(),
            protected_paths: crate::scope::ProtectedPaths::new(&config.github.repos),
            repositories: config.github.repos.clone(),
            limits: config.limits.clone(),
            silence_secs: config.allowances.agent_worker.silence_secs,
            no_progress_secs: config.allowances.agent_worker.no_progress_secs,
            store: config.store.clone(),
            records_root: records_root.to_owned(),
            materials_provider: None,
            worktrees: config.worktrees_root.clone(),
            tests: config.prismabuild.clone(),
            images: super::images::Images::new(&config.github.repos),
            hindsight: config.hindsight.clone(),
            memory: crate::memory::Feed::start(
                config.store.clone(),
                config.hindsight.clone(),
                recall_breaker.clone(),
                announcer.clone(),
            ),
            recall_breaker,
            announcer,
        })
    }

    /// Post memory hides and retirements on the operations issue (#439).
    pub(crate) fn with_operations(self, issue: crate::memory::OperationsIssue) -> Self {
        *self.announcer.lock().unwrap() = Some(Box::new(issue));
        self.memory.wake();
        self
    }

    pub(crate) fn with_materials(mut self, provider: crate::production::Materials) -> Self {
        self.materials_provider = Some(provider);
        self
    }

    pub fn source(&self) -> WorkerOutputSource {
        WorkerOutputSource::new(
            self.settings.runs.clone(),
            self.store.clone(),
            self.active.clone(),
        )
        .with_flusher(self.settings.launcher.clone(), self.settings.python.clone())
    }

    fn fence(&self, delivery: &Delivery) -> Result<(Store, Snapshot)> {
        let mut store = Store::open(&self.store)?;
        let snapshot = store.load()?;
        if snapshot.epoch.get() != delivery.epoch {
            return Err(Error::Invalid("worker epoch is fenced"));
        }
        Ok((store, snapshot))
    }

    fn root(&self, key: &str) -> PathBuf {
        run_root(&self.settings.runs, key)
    }

    fn request(
        &self,
        delivery: &Delivery,
        role: &str,
        store: &mut Store,
        snapshot: &Snapshot,
    ) -> Result<Value> {
        let request = self.build_request(delivery, role, store, snapshot);
        if let Err(Error::WorkerInputs(failure)) = &request {
            trace::input_failure(
                &self.settings,
                &delivery.key,
                json!({"node":delivery.node().0,"generation":delivery.generation,"kind":role}),
                json!({"repo":format!("{}/{}",delivery.repository.owner,delivery.repository.name),
                    "issue":delivery.issue,"request":delivery.node().0,"attempt":delivery.attempt,"round":delivery.round}),
                failure,
            );
        }
        request
    }

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

    /// The step's models in Q53 fallback order, configured model first.
    fn candidates(
        &self,
        delivery: &Delivery,
        role: &str,
        store: &mut Store,
        snapshot: &Snapshot,
    ) -> Result<Vec<&tl3_config::Model>> {
        // Only planners and reviewers need the world; other roles may run before it exists.
        let node = if matches!(role, "planner" | "reviewer") {
            let world = crate::snapshot::decode(snapshot)?;
            Some(world.nodes[delivery.node().0 as usize])
        } else {
            None
        };
        let rung = match &node {
            Some(n) if role == "planner" => usize::from(n.plan_retries),
            _ => usize::from(delivery.attempt.saturating_sub(1)),
        };
        // Review stays a different family from the maker that actually ran.
        let maker = if role == "reviewer" {
            store
                .worker_document(&delivery.node().0.to_string(), "maker_session")?
                .filter(|session| {
                    session["attempt"] == delivery.attempt
                        && session["replanned"] == delivery.replanned
                })
                .and_then(|session| session["model"].as_str().map(str::to_owned))
        } else {
            None
        };
        ladder::candidates(
            &self.models,
            role,
            rung,
            role == "reviewer" && node.is_some_and(|n| n.review_restarted),
            maker.as_deref(),
        )
    }

    fn materials(
        &self,
        delivery: &Delivery,
        store: &mut Store,
        snapshot: &Snapshot,
    ) -> Result<Value> {
        let mut ancestor = delivery.node();
        let mut linked_issue: Option<tl3_github::Issue> = None;
        let mut narrowing: Option<Value> = None;
        let mut origin = None;
        loop {
            let loaded = if ancestor != delivery.node()
                && let Some(provider) = &self.materials_provider
            {
                // A link preserves request context, never the ancestor's source or receipts.
                Some(provider.load(delivery)?)
            } else {
                store.worker_materials(&ancestor.0.to_string())?
            };
            if let Some(mut materials) = loaded {
                if ancestor != delivery.node()
                    && let Some(materials) = materials.as_object_mut()
                {
                    materials.remove("check_run");
                    materials.insert("request_origin".into(), json!(origin));
                }
                if let Some(issue) = linked_issue {
                    let answer = issue.body.unwrap_or_default();
                    let body = if let Some(narrowing) = &narrowing {
                        // #449: the answer is the scope; the original issue is context it may
                        // refer to. Later roles read the same text, so keep it for this request.
                        let (title, original, read) = self.original_issue(delivery, &materials);
                        let body = crate::narrowing::issue_body(
                            &answer,
                            &title,
                            &original,
                            narrowing
                                .get("question")
                                .filter(|question| !question.is_null()),
                            read.is_err(),
                        );
                        // A later narrowing reads this pristine copy, not the composed body.
                        materials["original_issue"] = json!({
                            "title": title, "body": original,
                            "read": read.err().unwrap_or_else(|| "github".into()),
                        });
                        Some(body)
                    } else {
                        None
                    };
                    let comments = if self.materials_provider.is_some() {
                        materials["issue"]["comments"].clone()
                    } else {
                        json!([])
                    };
                    materials["issue"] = json!({"number": issue.number, "title": issue.title,
                        "body": body.unwrap_or(answer), "author": "TL3", "comments": comments});
                    // Save source, orientation and linked issue together before any role starts.
                    store.put_worker_materials(&delivery.node().0.to_string(), &materials)?;
                }
                return Ok(materials);
            }
            if ancestor == delivery.node()
                && let Some(provider) = &self.materials_provider
                && !snapshot
                    .rows
                    .iter()
                    .any(|r| r.table == "request_links" && r.id == ancestor.0.to_string())
            {
                let materials = provider.load(delivery)?;
                store.put_worker_materials(&ancestor.0.to_string(), &materials)?;
                return Ok(materials);
            }
            let link = snapshot
                .rows
                .iter()
                .find(|r| r.table == "request_links" && r.id == ancestor.0.to_string())
                .ok_or(Error::Invalid("worker materials are missing"))?;
            if linked_issue.is_none() {
                linked_issue = Some(serde_json::from_value(link.value["issue"].clone())?);
                narrowing = link.value.get("narrowing").cloned();
            }
            let original: tl3_kernel::NodeId =
                serde_json::from_value(link.value["original"].clone())?;
            if original.0 >= ancestor.0 {
                return Err(Error::Invalid(
                    "request links must point to an earlier request",
                ));
            }
            origin.get_or_insert_with(|| original.0.to_string());
            ancestor = original;
        }
    }

    /// The original issue's title and body for a narrowed request, as GitHub has them now. If the
    /// read fails the stored copy stands in, the pristine one when this request's parent was itself
    /// narrowed, and the failure is returned so the agents and the stored materials say so.
    fn original_issue(
        &self,
        delivery: &Delivery,
        root: &Value,
    ) -> (String, String, std::result::Result<(), String>) {
        let read = match &self.materials_provider {
            Some(provider) => match provider.current_issue(delivery) {
                Ok(issue) => return (issue.title, issue.body.unwrap_or_default(), Ok(())),
                Err(error) => format!("GitHub read failed: {error}"),
            },
            None => {
                return {
                    let (title, body) = crate::narrowing::stored_original(root);
                    (title, body, Ok(()))
                };
            }
        };
        let (title, body) = crate::narrowing::stored_original(root);
        (title, body, Err(read))
    }

    /// Stop and prove termination before acknowledging teardown or a stall.
    pub fn stop_run(&self, key: &str, epoch: i64) -> Result<()> {
        if Store::open(&self.store)?.load()?.epoch.get() != epoch {
            return Err(Error::Invalid("worker stop epoch is fenced"));
        }
        if let Some(attempt) = stop_attempt(&self.root(key))? {
            facts::record(&mut Store::open(&self.store)?, key, &attempt)?;
            trace::flush(&attempt, &self.flusher());
        }
        Ok(())
    }

    fn flusher(&self) -> trace::Flusher<'_> {
        trace::Flusher {
            launcher: &self.settings.launcher,
            python: &self.settings.python,
        }
    }

    fn retire_sessions(&self, store: &mut Store) -> Result<()> {
        let sessions = self.settings.runs.join("sessions");
        let entries = match fs::read_dir(&sessions) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                eprintln!(
                    "worker session retirement retained {}: {error}",
                    sessions.display()
                );
                return Ok(());
            }
        };
        for entry in entries {
            let result = (|| -> Result<()> {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    return Ok(());
                }
                let Some(node) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<u64>().ok())
                else {
                    return Ok(());
                };
                if store.can_retire_worker_session(&node.to_string())? {
                    fs::remove_dir_all(entry.path())?;
                    File::open(&sessions)?.sync_all()?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                eprintln!(
                    "worker session retirement retained an entry in {}: {error}",
                    sessions.display()
                );
            }
        }
        Ok(())
    }

    fn retire_run(&self, key: &str, store: &mut Store) -> Result<()> {
        let root = self.root(key);
        stop_attempt(&root)?;
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries {
                let entry = entry?;
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("attempt-"))
                {
                    if facts::record(store, key, &entry.path()).is_err() {
                        eprintln!("worker completion facts unavailable; retirement continues");
                    }
                    match crate::memory::queue_attempt(store, key, &entry.path()) {
                        Ok(()) => {}
                        Err(Error::Invalid(_) | Error::Json(_)) => {
                            eprintln!(
                                "Hindsight memory record dropped for worker {key}; retirement continues"
                            );
                        }
                        Err(error) => return Err(error),
                    }
                    // #430: an unshipped span log outlives the run root.
                    trace::hold(&entry.path(), &self.settings.runs)?;
                }
            }
        }
        match fs::remove_dir_all(&root) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        // Keep the progress row as the durable cleanup queue until unlink is durable.
        let name = root.file_name().unwrap().to_string_lossy();
        remove_ready(&self.settings.runs.join(format!(".ready-{name}")))?;
        remove_ready(
            &self
                .settings
                .runs
                .join(".ready")
                .join(format!(".ready-{name}")),
        )?;
        File::open(&self.settings.runs)?.sync_all()?;
        File::open(self.settings.runs.join(".ready"))?.sync_all()?;
        store.forget_worker(key)?;
        self.active.remove_key(key);
        Ok(())
    }

    fn stop_node(&self, delivery: &Delivery) -> Result<()> {
        for key in self.active.keys(&delivery.node().0.to_string()) {
            self.stop_run(&key, delivery.epoch)?;
        }
        self.active
            .0
            .lock()
            .unwrap()
            .remove(&delivery.node().0.to_string());
        Ok(())
    }
}

fn stop_attempt(root: &Path) -> Result<Option<PathBuf>> {
    if let Some((_, attempt)) = latest(root)? {
        // Q49: stopping a live worker is deliberate, so its trace closes as
        // stopped rather than crashed (#430).
        let stopping = alive(&attempt.join("claim.json"))? || alive(&attempt.join("process.json"))?;
        // #372: the supervisor's own scope also holds every process the worker started.
        let scope = process::own_scope(&attempt.join("claim.json"))?;
        cancel(&attempt.join("claim.json"))?;
        cancel(&attempt.join("process.json"))?;
        if let Some(scope) = &scope {
            process::kill_scope(scope)?;
        }
        process::kill_and_wait(&attempt.join("process.json"))?;
        process::kill_and_wait(&attempt.join("claim.json"))?;
        if stopping {
            write(
                &attempt.join("stopped.json"),
                &json!({"finished_ms": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64}),
            )?;
        }
        return Ok(Some(attempt));
    }
    Ok(None)
}

impl Supervisor {
    fn launch(&mut self, delivery: &Delivery, resume_quota: bool) -> Result<Completion> {
        let (mut store, snapshot) = self.fence(delivery)?;
        let role = match delivery.output {
            Out::Effect {
                kind: EffectKind::StartRun(role),
                ..
            } => protocol::role(role),
            Out::Comment {
                kind: CommentKind::Classify | CommentKind::Postmortem,
                ..
            } => protocol::delivery_role(delivery),
            Out::Effect {
                kind: EffectKind::StopRun,
                ..
            } => {
                self.stop_node(delivery)?;
                return super::record::Records::new(&self.records_root)
                    .deliver(delivery, Completion::confirmed(vec![]));
            }
            _ => return Err(Error::Invalid("unsupported worker effect")),
        };
        let Some(role) = role else {
            return Err(Error::Invalid("unsupported worker role"));
        };
        if let Reconciliation::Found(completion) =
            super::record::Records::new(&self.records_root).reconcile(delivery)?
        {
            return Ok(*completion);
        }
        let root = self.root(&delivery.key);
        let previous = latest(&root)?;
        let result = match &previous {
            Some((_, attempt)) => inspect_recorded(attempt, delivery, &mut store)?,
            None => Reconciliation::Absent,
        };
        match result {
            Reconciliation::Found(completion) => {
                if !resume_quota
                    || !matches!(
                        completion.observations.as_slice(),
                        [Observation::Quota { .. }]
                    )
                {
                    if let Some((number, _)) = latest(&self.root(&delivery.key))? {
                        self.active.remove(delivery, number);
                    }
                    return Ok(*completion);
                }
            }
            Reconciliation::Pending => {
                if let Some((number, _)) = latest(&self.root(&delivery.key))? {
                    self.active.add(delivery, number);
                }
                return Ok(Completion::confirmed(vec![]));
            }
            Reconciliation::Absent => {}
        }
        let mut request = self.request(delivery, role, &mut store, &snapshot)?;
        if role == "ingest"
            && let Some(reply) = crate::protected_requests::ingest_preflight(&request["inputs"])
        {
            return super::record::Records::new(&self.records_root).deliver(
                delivery,
                Completion::confirmed(vec![Observation::WorkerReply {
                    node: delivery.node(),
                    generation: delivery.generation,
                    role: role.into(),
                    reply,
                    model: None,
                }]),
            );
        }

        let attempt_number = previous.map_or(0, |(n, _)| n + 1);
        request["prompt"] = crate::prompts::for_run(&mut store, &delivery.key, role)?;
        store.claim_worker(
            &delivery.key,
            &delivery.node().0.to_string(),
            delivery.epoch,
            attempt_number,
        )?;
        fs::create_dir_all(&root)?;
        if role != "maker" {
            fs::create_dir_all(root.join("worktree"))?;
        }
        if role == "reviewer" {
            review::checkout(&root.join("worktree"), delivery)?;
        } else if matches!(role, "ingest" | "audit" | "planner") {
            let materials = self.materials(delivery, &mut store, &snapshot)?;
            materialize(&root.join("worktree"), &materials)?;
        }
        let attempt = root.join(format!("attempt-{attempt_number:08}"));
        fs::create_dir(&attempt)?;
        // A postmortem's lessons reach Hindsight through its stored result,
        // not as a run summary.
        if !delivery.is_postmortem() {
            request["memory"] = json!({
                "summary_file": attempt.join("memory.json"),
                "repo": format!("{}/{}", delivery.repository.owner, delivery.repository.name),
                "issue": delivery.issue, "request": delivery.node().0.to_string(), "epoch": delivery.epoch,
            });
        }
        request["trace"]["log"] = json!(attempt.join(trace::LOG));
        request["trace"]["wake"] = json!(self.settings.runs.join(trace::HELD).join(trace::WAKE));
        let python = release_python(&self.settings.python)?;
        request["trace"]["completion"] = json!(attempt.join("facts.json"));
        request["trace"]["release"] = python
            .ancestors()
            .find(|path| path.file_name().is_some_and(|name| name == "workers"))
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .map(|name| json!(name.to_string_lossy()))
            .unwrap_or(Value::Null);
        // #372: the launcher moves the supervisor out of tl3d's cgroup, so a
        // restart or deploy leaves the run alive. A launcher failure fails the
        // launch; it never falls back to a supervisor tl3d's stop would kill.
        let mut command = match self.settings.launcher.split_first() {
            Some((launcher, args)) => {
                let mut command = Command::new(launcher);
                command.args(args).arg(&python);
                command
            }
            None => Command::new(&python),
        };
        isolate_credentials(&attempt, &mut request, &mut command)?;
        write(&attempt.join("request.json"), &request)?;
        write(&attempt.join("command.json"), &self.settings.command)?;
        // Deploy keeps every release a live attempt still runs from.
        write(&attempt.join("launch.json"), &json!({"python": python}))?;
        let mut script = File::create(attempt.join("supervisor.py"))?;
        script.write_all(include_bytes!("../../tools/worker_supervisor.py"))?;
        script.sync_all()?;
        File::open(&attempt)?.sync_all()?;
        File::open(&root)?.sync_all()?;
        let errors = File::create(attempt.join("launcher.log"))?;
        let mut child = command
            .arg(attempt.join("supervisor.py"))
            .arg(&attempt)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(errors)
            .spawn()?;
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut ready)?;
        if ready != "claimed\n" {
            reap(child);
            return Err(Error::Invalid(
                "worker launcher did not claim the attempt; inspect launcher.log",
            ));
        }
        self.active.add(delivery, attempt_number);
        reap(child);
        let name = root.file_name().unwrap().to_string_lossy();
        let ready = self
            .settings
            .runs
            .join(".ready")
            .join(format!(".ready-{name}"));
        write(&root.join("ready.json"), delivery)?;
        fs::rename(root.join("ready.json"), ready)?;
        File::open(self.settings.runs.join(".ready"))?.sync_all()?;
        Ok(Completion::confirmed(vec![]))
    }
}

impl Executor for Supervisor {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.launch(delivery, false)
    }

    fn probe(&mut self, _delivery: &Delivery) -> Result<bool> {
        // A recorded failed run is not a health check. Verify the supervisor can
        // still launch its configured worker runtime without making a model call.
        Ok(Command::new(&self.settings.python)
            .args(["-c", "import tl3_workers"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
            .success())
    }

    fn resume_after_quota(&mut self, delivery: &Delivery) -> Result<Option<Completion>> {
        self.launch(delivery, true).map(Some)
    }

    fn quota_resume_ms(&mut self, delivery: &Delivery, announced: u64, now: u64) -> Result<u64> {
        let role =
            protocol::delivery_role(delivery).ok_or(Error::Invalid("unsupported worker role"))?;
        let mut store = Store::open(&self.store)?;
        let snapshot = store.load()?;
        let mut resets = ladder::quota_resets(&snapshot);
        // Waits recorded before Q53 name their model only in the attempt itself.
        if let Some((_, attempt)) = latest(&self.root(&delivery.key))?
            && let Some(request) = read(&attempt.join("request.json"))?
            && let Some(model) = request["model"]["name"].as_str()
        {
            let reset = resets.entry(model.to_owned()).or_default();
            *reset = (*reset).max(announced);
        }
        let candidates = self.candidates(delivery, role, &mut store, &snapshot)?;
        let limited: Vec<u64> = candidates
            .iter()
            .map(|model| {
                resets
                    .get(&model.name)
                    .copied()
                    .filter(|reset| *reset > now)
            })
            .collect::<Option<_>>()
            .unwrap_or_default();
        // Wait only when every model is out, and then for the earliest reset.
        Ok(if limited.len() < candidates.len() {
            now
        } else {
            limited.into_iter().min().unwrap_or(announced)
        })
    }

    fn look(&mut self, delivery: &Delivery) -> Result<Look> {
        // A postmortem runs long, so its looks read progress like a node's run.
        if !delivery.is_postmortem()
            && !matches!(delivery.output, Out::Effect { kind: EffectKind::StartRun(role), .. } if protocol::role(role).is_some())
        {
            return Ok(match self.reconcile(delivery)? {
                Reconciliation::Found(c) => Look::Found(c),
                Reconciliation::Absent => Look::Absent,
                Reconciliation::Pending => Look::Pending,
            });
        }
        match self.reconcile(delivery)? {
            Reconciliation::Found(c) => Ok(Look::Found(c)),
            Reconciliation::Absent => Ok(Look::Absent),
            Reconciliation::Pending => {
                let row = Store::open(&self.store)?
                    .worker_progress(Some(&delivery.node().0.to_string()))?
                    .into_iter()
                    .find(|r| r.key == delivery.key);
                Ok(match row {
                    Some(row) => match row.external_wait {
                        Some(wait) => Look::Waiting {
                            wait,
                            checkout: if protocol::delivery_role(delivery) == Some("maker") {
                                super::worktrees::checkout_path(&self.worktrees, delivery)
                            } else {
                                self.root(&delivery.key).join("worktree")
                            },
                        },
                        None => Look::Alive {
                            progress: format!("{}:{}", row.attempt, row.output_offset),
                        },
                    },
                    None => Look::Pending,
                })
            }
        }
    }

    fn stop_stalled(&mut self, delivery: &Delivery) -> Result<Completion> {
        if let Reconciliation::Found(c) = self.reconcile(delivery)? {
            return Ok(*c);
        }
        self.stop_run(&delivery.key, delivery.epoch)?;
        if let Some((number, _)) = latest(&self.root(&delivery.key))? {
            self.active.remove(delivery, number);
        }
        let result = match latest(&self.root(&delivery.key))? {
            Some((_, attempt)) => {
                inspect_recorded(&attempt, delivery, &mut Store::open(&self.store)?)?
            }
            None => Reconciliation::Absent,
        };
        Ok(match result {
            Reconciliation::Found(c) => *c,
            Reconciliation::Absent => failed(delivery),
            Reconciliation::Pending => return Err(Error::Invalid("stalled worker has not ended")),
        })
    }

    fn retire(&mut self, store: &mut Store) -> Result<()> {
        let mut failure = None;
        for key in store.retired_workers()? {
            if let Err(error) = self.retire_run(&key, store) {
                if matches!(
                    error,
                    Error::Json(_) | Error::Invalid("worker record belongs to another machine")
                ) {
                    store.block_worker_retirement(&key)?;
                    self.active.remove_key(&key);
                }
                eprintln!("worker retirement retained {key}: {error}");
                if matches!(error, Error::Store(_)) && failure.is_none() {
                    failure = Some(error);
                }
            }
        }
        self.memory.wake();
        self.retire_sessions(store)?;
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        match delivery.output {
            Out::Effect {
                kind: EffectKind::StartRun(role),
                ..
            } if protocol::role(role).is_some() => self.reconcile_run(delivery),
            Out::Comment {
                kind: CommentKind::Classify | CommentKind::Postmortem,
                ..
            } => self.reconcile_run(delivery),
            Out::Effect {
                kind: EffectKind::StopRun,
                ..
            } => super::record::Records::new(&self.records_root).reconcile(delivery),
            _ => Err(Error::Invalid("unsupported worker effect")),
        }
    }
}

impl Supervisor {
    fn reconcile_run(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        if let found @ Reconciliation::Found(_) =
            super::record::Records::new(&self.records_root).reconcile(delivery)?
        {
            return Ok(found);
        }

        let result = match latest(&self.root(&delivery.key))? {
            Some((_, attempt)) => {
                inspect_recorded(&attempt, delivery, &mut Store::open(&self.store)?)?
            }
            None => Reconciliation::Absent,
        };
        if !matches!(result, Reconciliation::Pending)
            && let Some((number, _)) = latest(&self.root(&delivery.key))?
        {
            self.active.remove(delivery, number);
        }
        Ok(result)
    }
}

/// Pin a run to the release its Python path names at launch. The path's
/// directories may pass through a release link that a later deploy switches;
/// the interpreter itself stays a link, because a venv is found from its path.
fn release_python(python: &Path) -> Result<PathBuf> {
    match (python.parent(), python.file_name()) {
        (Some(directory), Some(name)) => Ok(fs::canonicalize(directory)?.join(name)),
        _ => Err(Error::Invalid("workers.python must name a file")),
    }
}

fn reap(mut child: Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

pub(super) fn run_root(root: &Path, key: &str) -> PathBuf {
    root.join(key.bytes().map(|b| format!("{b:02x}")).collect::<String>())
}

pub(super) fn latest(root: &Path) -> Result<Option<(i64, PathBuf)>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut latest = None;
    for entry in entries {
        let entry = entry?;
        if let Some(number) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.strip_prefix("attempt-"))
            .and_then(|s| s.parse::<i64>().ok())
            && latest.as_ref().is_none_or(|(n, _)| number > *n)
        {
            latest = Some((number, entry.path()));
        }
    }
    Ok(latest)
}

pub(crate) fn failed(delivery: &Delivery) -> Completion {
    failed_with(delivery, "worker_failed")
}

/// A failed run's completion. A postmortem names a fixed failure class, never
/// the worker's own text.
fn failed_with(delivery: &Delivery, class: &str) -> Completion {
    if delivery.is_postmortem() {
        return Completion::confirmed(vec![Observation::Postmortem {
            key: delivery.key.clone(),
            reply: None,
            judge_failure: None,
            failure: Some(class.into()),
        }]);
    }
    if delivery.is_classifier() {
        return Completion::confirmed(vec![Observation::Classified {
            key: delivery.key.clone(),
            reply: None,
        }]);
    }
    Completion::confirmed(vec![Observation::Kernel(Event::Reply {
        node: delivery.node(),
        generation: delivery.generation,
        reply: Reply::RunFailed,
    })])
}

fn inspect_recorded(
    attempt: &Path,
    delivery: &Delivery,
    store: &mut Store,
) -> Result<Reconciliation> {
    let result = inspect(attempt, delivery)?;
    if !matches!(result, Reconciliation::Pending) {
        facts::record(store, &delivery.key, attempt)?;
    }
    Ok(result)
}

/// Inspect one attempt directory, rather than the run root or historical attempts.
pub(super) fn inspect(attempt: &Path, delivery: &Delivery) -> Result<Reconciliation> {
    // Claim cancellation closes the spawn-before-record crash window atomically.
    // A delayed launcher must lose this claim before it can start any worker.
    cancel(&attempt.join("claim.json"))?;
    if alive(&attempt.join("claim.json"))? || alive(&attempt.join("process.json"))? {
        return Ok(Reconciliation::Pending);
    }
    // Fence a not-yet-scheduled worker wrapper before declaring it absent.
    cancel(&attempt.join("process.json"))?;
    if alive(&attempt.join("process.json"))? {
        return Ok(Reconciliation::Pending);
    }
    let Some(exit) = read(&attempt.join("exit.json"))? else {
        return Ok(Reconciliation::Absent);
    };
    if exit["code"] != 0 {
        if delivery.is_postmortem() {
            // #486: a run its own deadline stopped names that class.
            let summary = read(&attempt.join("memory.json")).ok().flatten();
            let reason = summary.as_ref().and_then(|summary| summary.get("reason"));
            if reason.is_some_and(|reason| reason["failure_class"] == "PostmortemDeadline") {
                return Ok(Reconciliation::Found(Box::new(failed_with(
                    delivery, "deadline",
                ))));
            }
            let judge_failure = reason.map(|reason| {
                let class = serde_json::from_value(reason["failure_class"].clone())
                    .unwrap_or(tl3_store::JudgeFailureClass::Other);
                let http_status = reason["failure_http_status"]
                    .as_str()
                    .filter(|status| {
                        status.len() == 3 && status.bytes().all(|b| b.is_ascii_digit())
                    })
                    .and_then(|status| status.parse::<u16>().ok())
                    .and_then(|status| tl3_store::JudgeHttpStatus::try_from(status).ok());
                tl3_store::JudgeFailure { class, http_status }
            });
            return Ok(Reconciliation::Found(Box::new(Completion::confirmed(
                vec![Observation::Postmortem {
                    key: delivery.key.clone(),
                    reply: None,
                    failure: Some("worker_failed".into()),
                    judge_failure,
                }],
            ))));
        }
        let mut completion = failed(delivery);
        if delivery.job_table().is_none()
            && let Some(summary) = read(&attempt.join("memory.json")).ok().flatten()
            && let Some(reason) = summary.get("reason")
            && let (Some(class), Some(message)) = (
                reason["failure_class"].as_str(),
                reason["failure_message"].as_str(),
            )
            && !class.is_empty()
            && class.len() <= 64
            && class
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && !message.is_empty()
            && message.len() <= 512
            && !message.chars().any(char::is_control)
        {
            completion.observations = vec![Observation::WorkerFailed {
                node: delivery.node(),
                generation: delivery.generation,
                failure_class: class.into(),
                failure_message: message.into(),
            }];
        }
        return Ok(Reconciliation::Found(Box::new(completion)));
    }
    let observation = match fs::read(attempt.join("stdout.jsonl")) {
        Ok(bytes) => read(&attempt.join("request.json"))?
            .ok_or(Error::Invalid("worker request is missing"))
            .and_then(|request| {
                let mut observation = source::parse_reply(&bytes, delivery, &request["inputs"])?;
                if let Observation::Quota { model, .. } | Observation::WorkerReply { model, .. } =
                    &mut observation
                {
                    *model = request["model"]["name"].as_str().map(str::to_owned);
                }
                Ok(observation)
            }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(Error::Invalid("worker output is missing"))
        }
        Err(e) => return Err(e.into()),
    };
    Ok(Reconciliation::Found(Box::new(match observation {
        Ok(observation) => Completion::confirmed(vec![observation]),
        Err(_) => failed_with(delivery, "invalid_reply"),
    })))
}

fn remove_ready(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn watched_deliveries(store: &mut Store) -> Result<Vec<Delivery>> {
    let mut deliveries = Vec::new();
    for (key, value) in store.worker_watches()? {
        match serde_json::from_value::<Delivery>(value) {
            Ok(delivery) if delivery.key == key => {
                if protocol::delivery_role(&delivery).is_some() {
                    deliveries.push(delivery);
                }
            }
            Ok(_) => eprintln!("worker startup skipped mismatched watch {key}"),
            Err(error) => eprintln!("worker startup skipped {key}: {error}"),
        }
    }
    Ok(deliveries)
}

fn materialize(root: &Path, materials: &Value) -> Result<()> {
    if let Some(base) = materials.get("base_checkout") {
        let repository = Path::new(
            base["repository"]
                .as_str()
                .ok_or(Error::Invalid("base repository missing"))?,
        );
        let commit = CommitId::new(
            base["commit"]
                .as_str()
                .ok_or(Error::Invalid("base commit missing"))?,
        )?;
        // A retry must discard a partial clone and return to the same committed base.
        if root.exists() {
            fs::remove_dir_all(root)?;
        }
        Worktrees::open(repository, repository)?.clone_checkout(root, &commit)?;
        return Ok(());
    }
    let repo = materials.get("repo").unwrap_or(materials);
    let files = repo["file_list"]
        .as_array()
        .ok_or(Error::Invalid("repository file list missing"))?;
    let materials: HashMap<_, _> = repo["materials"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| Some((m["path"].as_str()?, m["content"].as_str()?)))
        .collect();
    for path in files {
        let path = path
            .as_str()
            .ok_or(Error::Invalid("invalid repository path"))?;
        if path.is_empty()
            || path.contains(['\\', '\0'])
            || path
                .split('/')
                .any(|p| p.is_empty() || matches!(p, "." | ".." | ".git"))
        {
            return Err(Error::Invalid(
                "repository path must be canonical and confined",
            ));
        }
        // Missing text can represent a binary file. Keep it distinct from a new
        // file so declaring it fails measurement instead of silently counting zero.
        let content = materials.get(path).copied().unwrap_or("\0");
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap())?;
        fs::write(target, content)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "workers/materialize_tests.rs"]
mod materialize_tests;

#[cfg(test)]
#[path = "workers/active_tests.rs"]
mod active_tests;

#[cfg(test)]
mod release_tests {
    use super::release_python;
    use std::fs;

    #[test]
    fn launch_pins_the_release_a_switched_link_names() {
        let root = tempfile::tempdir().unwrap();
        for release in ["one", "two"] {
            let bin = root
                .path()
                .join("releases")
                .join(release)
                .join("workers/.venv/bin");
            fs::create_dir_all(&bin).unwrap();
            std::os::unix::fs::symlink("/usr/bin/python3", bin.join("python")).unwrap();
        }
        let current = root.path().join("current");
        std::os::unix::fs::symlink(root.path().join("releases/one"), &current).unwrap();
        let python = current.join("workers/.venv/bin/python");
        let pinned = release_python(&python).unwrap();
        let releases = fs::canonicalize(root.path().join("releases")).unwrap();
        assert_eq!(pinned, releases.join("one/workers/.venv/bin/python"));
        // A deploy switches the link; the pinned path still names release one.
        fs::remove_file(&current).unwrap();
        std::os::unix::fs::symlink(root.path().join("releases/two"), &current).unwrap();
        assert_eq!(pinned, releases.join("one/workers/.venv/bin/python"));
        assert_eq!(
            release_python(&python).unwrap(),
            releases.join("two/workers/.venv/bin/python")
        );
    }
}
