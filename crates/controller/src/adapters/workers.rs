use crate::executor::Output as Out;
pub mod writer;
pub use writer::{FakeWriter, Writer};

mod fake;
mod json;
mod process;
pub(crate) mod protocol;
pub(crate) mod reproducing;
pub(crate) mod review;
mod source;
#[cfg(test)]
mod startup_tests;
pub use fake::Fake;
pub use source::WorkerOutputSource;

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
    sync::{Arc, Mutex},
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
impl ActiveRuns {
    fn add(&self, delivery: &Delivery, attempt: i64) {
        self.0
            .lock()
            .unwrap()
            .entry(if delivery.is_classifier() {
                format!("comment:{}", delivery.node().0)
            } else {
                delivery.node().0.to_string()
            })
            .or_default()
            .entry(delivery.key.clone())
            .and_modify(|current| *current = (*current).max(attempt))
            .or_insert(attempt);
    }
    fn remove(&self, delivery: &Delivery, attempt: i64) {
        let mut nodes = self.0.lock().unwrap();
        let node = if delivery.is_classifier() {
            format!("comment:{}", delivery.node().0)
        } else {
            delivery.node().0.to_string()
        };
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
    settings: tl3_config::Workers,
    models: tl3_config::Models,
    planning: tl3_config::Planning,
    limits: tl3_config::Limits,
    store: tl3_config::Store,
    records_root: PathBuf,
    materials_provider: Option<crate::production::Materials>,
    active: ActiveRuns,
    worktrees: PathBuf,
    tests: tl3_config::PrismaBuild,
    hindsight: tl3_config::Hindsight,
    memory: crate::memory::Feed,
}

impl Supervisor {
    pub fn new(config: &tl3_config::Config, records_root: &Path) -> Result<Self> {
        fs::create_dir_all(config.workers.runs.join(".ready"))?;
        fs::create_dir_all(records_root)?;
        let active = ActiveRuns::default();
        let mut store = Store::open(&config.store)?;
        for delivery in watched_deliveries(&mut store)? {
            let root = run_root(&config.workers.runs, &delivery.key);
            match latest(&root).and_then(|attempt| match attempt {
                Some((_, path)) => inspect(&path, &delivery),
                None => Ok(Reconciliation::Absent),
            }) {
                Ok(Reconciliation::Pending) => {
                    active.add(&delivery, latest(&root)?.expect("pending attempt").0)
                }
                Ok(_) => {}
                Err(error) => eprintln!("worker startup skipped {}: {error}", delivery.key),
            }
        }
        Ok(Self {
            active,
            settings: config.workers.clone(),
            models: config.models.clone(),
            planning: config.planning.clone(),
            limits: config.limits.clone(),
            store: config.store.clone(),
            records_root: records_root.to_owned(),
            materials_provider: None,
            worktrees: config.worktrees_root.clone(),
            tests: config.prismabuild.clone(),
            hindsight: config.hindsight.clone(),
            memory: crate::memory::Feed::start(config.store.clone(), config.hindsight.clone()),
        })
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
        let node = delivery.node().0.to_string();
        let mut materials =
            if let Some(crate::steering::Task::Classify { inputs, .. }) = &delivery.steering {
                inputs.clone()
            } else {
                self.materials(delivery, store, snapshot)?
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
            let base = match delivery
                .checks
                .as_ref()
                .map(|c| c.base_sha.clone())
                .or_else(|| {
                    materials["check_run"]["base_sha"]
                        .as_str()
                        .map(str::to_owned)
                }) {
                Some(base) => base,
                None => {
                    let output = Command::new("git")
                        .arg("-C")
                        .arg(&checkout)
                        .args(["rev-parse", "HEAD"])
                        .output()?;
                    if !output.status.success() {
                        return Err(Error::Invalid("maker checkout has no base"));
                    }
                    String::from_utf8(output.stdout)
                        .map_err(|_| Error::Invalid("invalid maker base"))?
                        .trim()
                        .to_owned()
                }
            };
            let checks = materials["maker"]["plan"]["checks"]
                .as_array()
                .ok_or(Error::Invalid("maker plan needs checks"))?
                .iter()
                .map(|c| json!({"name":c["name"],"kind":c["kind"],"tests":c["tests"]}))
                .collect::<Vec<_>>();
            materials["check_run"] = json!({"repository":checkout,"base_sha":base,"plan":checks});
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
                    inputs["feedback"] = feedback;
                }
            }
            inputs["attempt"] = json!(delivery.attempt);
            inputs["round"] = json!(delivery.round);
            inputs.as_object_mut().unwrap().remove("reviewed_sha");
            if delivery.round == 1
                && let Some(head) = &delivery.head_sha
                && inputs["feedback"].as_array().is_some_and(|items| {
                    items.iter().any(|item| {
                        item["kind"] == "review_finding" && item["reviewed_sha"] == *head
                    })
                })
            {
                inputs["reviewed_sha"] = json!(head);
            }
            inputs["plan"]["files"] = json!(delivery.paths);
            inputs["context_size"] = json!(self.limits.context_tokens);
            inputs["fit_tokens"] = json!(self.limits.fit_tokens);
            inputs["tests"] =
                json!({"image": self.tests.container_image, "tools_path": self.tests.tools_dir});
            inputs["hindsight"] =
                json!({"endpoint": self.hindsight.endpoint, "bank": self.hindsight.bank});
            super::worktrees::checkout_path(&self.worktrees, delivery)
        } else {
            self.root(&delivery.key).join("worktree")
        };
        let forecast = json!({"fit_tokens": self.limits.fit_tokens, "context_tokens": self.limits.context_tokens, "overhead_tokens": self.planning.overhead_tokens});
        let mut rung = usize::from(delivery.attempt.saturating_sub(1));
        if role == "reviewer" {
            inputs = review::inputs(delivery, store, snapshot)?;
            inputs["context_size"] = json!(self.limits.context_tokens);
        } else if role == "planner" {
            let world = crate::snapshot::decode(snapshot)?;
            let n = &world.nodes[delivery.node().0 as usize];
            rung = usize::from(n.plan_retries);
            let ingest = store
                .worker_document(&node, "briefs")?
                .ok_or(Error::Invalid("planner needs a brief"))?;
            let findings = store
                .worker_document(&node, "findings")?
                .unwrap_or(json!([]));
            inputs = json!({"ingest": ingest, "must_split": matches!(n.phase, tl3_kernel::Phase::Planning { must_split: true }), "findings": findings, "protected_paths": self.planning.protected_paths, "forecast": forecast});
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
        if role != "reviewer" {
            inputs
                .as_object_mut()
                .ok_or(Error::Invalid("worker materials must be an object"))?
                .remove("standing_rules");
        }
        protocol::validate_inputs(role, &inputs)?;
        let ladder: &[tl3_config::Model] = match role {
            "classifier" => &self.models.comment_classifier,
            "ingest" => &self.models.ingest,
            "audit" => &self.models.audit,
            "maker" => &self.models.maker,
            "planner" => &self.models.planner,
            "reviewer" => &self.models.reviewer,
            _ => return Err(Error::Invalid("unsupported worker role")),
        };
        let model = if role == "reviewer" {
            let maker = &self.models.maker[rung.min(self.models.maker.len() - 1)];
            self.models
                .reviewer_for(&maker.family)
                .ok_or(Error::Invalid("reviewer needs a different model family"))?
        } else {
            &ladder[rung.min(ladder.len() - 1)]
        };
        let mut request = json!({
            "protocol": 1,
            "operation": {"node": delivery.node().0, "generation": delivery.generation, "kind": role},
            "role": role, "model": self.models.worker_model(model), "key_file": self.models.key_file,
            "inputs": inputs, "worktree": worktree,
            "hindsight_key_file": self.hindsight.api_key_file,
            "trace": {
                "repo": format!("{}/{}", delivery.repository.owner, delivery.repository.name),
                "issue": delivery.issue, "request": node,
                "attempt": delivery.attempt, "round": delivery.round,
            },
        });
        if role == "maker" {
            request["session_file"] = json!(
                self.settings
                    .runs
                    .join("sessions")
                    .join(&node)
                    .join(format!("plan-{}", u8::from(delivery.replanned)))
                    .join(format!("attempt-{}.json", delivery.attempt))
            );
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
            let needs_files = session.is_some_and(|session| session["needs_files"] == true);
            request["resume_session"] = json!(delivery.round == 1 || needs_files);
        } else if role == "reviewer" {
            request["session_file"] = json!(
                self.settings
                    .runs
                    .join("sessions")
                    .join(&node)
                    .join(format!("plan-{}", u8::from(delivery.replanned)))
                    .join(format!("reviewer-attempt-{}.json", delivery.attempt))
            );
            let session = review::session(delivery, store)?;
            if let Some(session) = &session {
                request["last_accepted_generation"] = json!(session["generation"].as_u64().ok_or(
                    Error::Invalid("reviewer session has no accepted generation")
                )?);
            }
            request["resume_session"] = json!(delivery.round == 1 && session.is_some());
        }
        Ok(request)
    }

    fn materials(
        &self,
        delivery: &Delivery,
        store: &mut Store,
        snapshot: &Snapshot,
    ) -> Result<Value> {
        let mut ancestor = delivery.node();
        let mut linked_issue: Option<tl3_github::Issue> = None;
        loop {
            if let Some(mut materials) = store.worker_materials(&ancestor.0.to_string())? {
                if let Some(issue) = linked_issue {
                    materials["issue"] = json!({"number": issue.number, "title": issue.title,
                        "body": issue.body.unwrap_or_default(), "author": "TL3", "comments": []});
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
            }
            let original: tl3_kernel::NodeId =
                serde_json::from_value(link.value["original"].clone())?;
            if original.0 >= ancestor.0 {
                return Err(Error::Invalid(
                    "request links must point to an earlier request",
                ));
            }
            ancestor = original;
        }
    }

    /// Stop and prove termination before acknowledging teardown or a stall.
    pub fn stop_run(&self, key: &str, epoch: i64) -> Result<()> {
        if Store::open(&self.store)?.load()?.epoch.get() != epoch {
            return Err(Error::Invalid("worker stop epoch is fenced"));
        }
        stop_attempt(&self.root(key))?;
        Ok(())
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
                    match crate::memory::queue_attempt(store, key, &entry.path()) {
                        Ok(()) => {}
                        Err(Error::Invalid(_) | Error::Json(_)) => {
                            eprintln!(
                                "Hindsight memory record dropped for worker {key}; retirement continues"
                            );
                        }
                        Err(error) => return Err(error),
                    }
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

fn stop_attempt(root: &Path) -> Result<()> {
    if let Some((_, attempt)) = latest(root)? {
        cancel(&attempt.join("claim.json"))?;
        cancel(&attempt.join("process.json"))?;
        process::kill_and_wait(&attempt.join("process.json"))?;
        process::kill_and_wait(&attempt.join("claim.json"))?;
    }
    Ok(())
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
                kind: CommentKind::Classify,
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
        let root = self.root(&delivery.key);
        let previous = latest(&root)?;
        let result = match &previous {
            Some((_, attempt)) => inspect(attempt, delivery)?,
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
                    return Ok(completion);
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
        let attempt_number = previous.map_or(0, |(n, _)| n + 1);
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
        } else if matches!(role, "ingest" | "planner") {
            let materials = self.materials(delivery, &mut store, &snapshot)?;
            materialize(&root.join("worktree"), &materials["repo"])?;
        }
        let attempt = root.join(format!("attempt-{attempt_number:08}"));
        fs::create_dir(&attempt)?;
        request["memory"] = json!({
            "summary_file": attempt.join("memory.json"),
            "repo": format!("{}/{}", delivery.repository.owner, delivery.repository.name),
            "issue": delivery.issue, "request": delivery.node().0.to_string(), "epoch": delivery.epoch,
        });
        let mut command = Command::new(&self.settings.python);
        isolate_credentials(&attempt, &mut request, &mut command)?;
        write(&attempt.join("request.json"), &request)?;
        write(&attempt.join("command.json"), &self.settings.command)?;
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

    fn look(&mut self, delivery: &Delivery) -> Result<Look> {
        if !matches!(delivery.output, Out::Effect { kind: EffectKind::StartRun(role), .. } if protocol::role(role).is_some())
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
            return Ok(c);
        }
        self.stop_run(&delivery.key, delivery.epoch)?;
        if let Some((number, _)) = latest(&self.root(&delivery.key))? {
            self.active.remove(delivery, number);
        }
        let result = match latest(&self.root(&delivery.key))? {
            Some((_, attempt)) => inspect(&attempt, delivery)?,
            None => Reconciliation::Absent,
        };
        Ok(match result {
            Reconciliation::Found(c) => c,
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
                kind: CommentKind::Classify,
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
        let result = match latest(&self.root(&delivery.key))? {
            Some((_, attempt)) => inspect(&attempt, delivery)?,
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
        return Ok(Reconciliation::Found(failed(delivery)));
    }
    let observation = match fs::read(attempt.join("stdout.jsonl")) {
        Ok(bytes) => read(&attempt.join("request.json"))?
            .ok_or(Error::Invalid("worker request is missing"))
            .and_then(|request| source::parse_reply(&bytes, delivery, &request["inputs"])),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(Error::Invalid("worker output is missing"))
        }
        Err(e) => return Err(e.into()),
    };
    Ok(Reconciliation::Found(match observation {
        Ok(observation) => Completion::confirmed(vec![observation]),
        Err(_) => failed(delivery),
    }))
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

fn materialize(root: &Path, repo: &Value) -> Result<()> {
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
