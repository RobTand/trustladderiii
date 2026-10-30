pub mod writer;
pub use writer::{FakeWriter, Writer};

mod fake;
mod json;
mod process;
pub(crate) mod protocol;
mod source;
pub use fake::Fake;
pub use source::WorkerOutputSource;

use crate::{
    Error, Result,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation},
};
use process::{alive, cancel, read, write};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
};
use tl3_kernel::{EffectKind, Event, Out, Reply};
use tl3_store::{Snapshot, Store};

#[derive(Clone, Default)]
pub(super) struct ActiveRuns(Arc<Mutex<BTreeMap<String, BTreeSet<String>>>>);
impl ActiveRuns {
    fn add(&self, delivery: &Delivery) {
        self.0
            .lock()
            .unwrap()
            .entry(delivery.node().0.to_string())
            .or_default()
            .insert(delivery.key.clone());
    }
    fn remove(&self, delivery: &Delivery) {
        let mut nodes = self.0.lock().unwrap();
        let node = delivery.node().0.to_string();
        if let Some(keys) = nodes.get_mut(&node) {
            keys.remove(&delivery.key);
            if keys.is_empty() {
                nodes.remove(&node);
            }
        }
    }
    fn keys(&self, node: &str) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .get(node)
            .map(|keys| keys.iter().cloned().collect())
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
    fake_root: PathBuf,
    active: ActiveRuns,
    worktrees: PathBuf,
    tests: tl3_config::PrismaBuild,
    hindsight: tl3_config::Hindsight,
}

impl Supervisor {
    pub fn new(config: &tl3_config::Config, fake_root: &Path) -> Result<Self> {
        fs::create_dir_all(&config.workers.runs)?;
        fs::create_dir_all(fake_root)?;
        let active = ActiveRuns::default();
        for entry in fs::read_dir(&config.workers.runs)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let root = entry.path();
            if let Some(value) = read(&root.join("delivery.json"))? {
                let delivery: Delivery = serde_json::from_value(value)?;
                if matches!(inspect(&root, &delivery)?, Reconciliation::Pending) {
                    active.add(&delivery);
                }
            }
        }
        Ok(Self {
            active,
            settings: config.workers.clone(),
            models: config.models.clone(),
            planning: config.planning.clone(),
            limits: config.limits.clone(),
            store: config.store.clone(),
            fake_root: fake_root.to_owned(),
            worktrees: config.worktrees_root.clone(),
            tests: config.prismabuild.clone(),
            hindsight: config.hindsight.clone(),
        })
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
        let materials = store
            .worker_materials(&node)?
            .ok_or(Error::Invalid("worker materials are missing"))?;
        let mut inputs = materials.get(role).unwrap_or(&materials).clone();
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
            inputs["plan"]["files"] = json!(delivery.paths);
            inputs["context_size"] = json!(self.limits.context_tokens);
            inputs["fit_tokens"] = json!(self.limits.fit_tokens);
            inputs["tests"] =
                json!({"image": self.tests.container_image, "tools_path": self.tests.tools_dir});
            inputs["hindsight"] =
                json!({"endpoint": self.hindsight.endpoint, "bank": self.hindsight.bank});
            let encoded = |name: &str| {
                name.to_ascii_lowercase()
                    .bytes()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            };
            self.worktrees
                .join(encoded(&delivery.repository.owner))
                .join(encoded(&delivery.repository.name))
                .join(format!(
                    "issue-{}-node-{}",
                    delivery.issue,
                    delivery.node().0
                ))
        } else {
            self.root(&delivery.key).join("worktree")
        };
        let forecast = json!({"fit_tokens": self.limits.fit_tokens, "context_tokens": self.limits.context_tokens, "overhead_tokens": self.planning.overhead_tokens});
        let mut rung = usize::from(delivery.attempt.saturating_sub(1));
        if role == "planner" {
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
        protocol::validate_inputs(role, &inputs)?;
        let ladder: &[tl3_config::Model] = match role {
            "ingest" => &self.models.ingest,
            "audit" => &self.models.audit,
            "maker" => &self.models.maker,
            "planner" => &self.models.planner,
            _ => return Err(Error::Invalid("unsupported worker role")),
        };
        let model = &ladder[rung.min(ladder.len() - 1)];
        let mut request = json!({
            "protocol": 1,
            "operation": {"node": delivery.node().0, "generation": delivery.generation, "kind": role},
            "role": role, "model": self.models.worker_model(model), "key_file": self.models.key_file,
            "inputs": inputs, "worktree": worktree,
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
            let needs_files =
                store
                    .worker_document(&node, "maker_session")?
                    .is_some_and(|session| {
                        session["needs_files"] == true
                            && session["attempt"] == delivery.attempt
                            && session["replanned"] == delivery.replanned
                    });
            request["resume_session"] = json!(delivery.round == 1 || needs_files);
        }
        Ok(request)
    }

    /// Stop and prove termination before acknowledging teardown or a stall.
    pub fn stop_run(&self, key: &str, epoch: i64) -> Result<()> {
        if Store::open(&self.store)?.load()?.epoch.get() != epoch {
            return Err(Error::Invalid("worker stop epoch is fenced"));
        }
        let root = self.root(key);
        if let Some((_, attempt)) = latest(&root)? {
            cancel(&attempt.join("claim.json"))?;
            cancel(&attempt.join("process.json"))?;
            process::kill_and_wait(&attempt.join("process.json"))?;
            process::kill_and_wait(&attempt.join("claim.json"))?;
        }
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

impl Executor for Supervisor {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        let (mut store, snapshot) = self.fence(delivery)?;
        let role = match delivery.output {
            Out::Effect {
                kind: EffectKind::StartRun(role),
                ..
            } => protocol::role(role),
            Out::Effect {
                kind: EffectKind::StopRun,
                ..
            } => {
                self.stop_node(delivery)?;
                return Fake::new(&self.fake_root).execute(delivery);
            }
            _ => return Err(Error::Invalid("unsupported worker effect")),
        };
        let Some(role) = role else {
            return Fake::new(&self.fake_root).execute(delivery);
        };
        match inspect(&self.root(&delivery.key), delivery)? {
            Reconciliation::Found(completion) => {
                self.active.remove(delivery);
                return Ok(completion);
            }
            Reconciliation::Pending => {
                self.active.add(delivery);
                return Ok(Completion::confirmed(vec![]));
            }
            Reconciliation::Absent => {}
        }
        let root = self.root(&delivery.key);
        fs::create_dir_all(root.join("worktree"))?;
        let request = self.request(delivery, role, &mut store, &snapshot)?;
        if matches!(role, "ingest" | "planner") {
            let materials = store
                .worker_materials(&delivery.node().0.to_string())?
                .ok_or(Error::Invalid("worker materials missing"))?;
            materialize(&root.join("worktree"), &materials["repo"])?;
        }
        let attempt_number = latest(&root)?.map_or(0, |(n, _)| n + 1);
        let attempt = root.join(format!("attempt-{attempt_number:08}"));
        fs::create_dir(&attempt)?;
        write(&attempt.join("request.json"), &request)?;
        write(&attempt.join("command.json"), &self.settings.command)?;
        let mut script = File::create(attempt.join("supervisor.py"))?;
        script.write_all(include_bytes!("../../tools/worker_supervisor.py"))?;
        script.sync_all()?;
        write(&root.join("delivery.json"), delivery)?;
        File::open(&attempt)?.sync_all()?;
        File::open(&root)?.sync_all()?;
        let errors = File::create(attempt.join("launcher.log"))?;
        store.claim_worker(
            &delivery.key,
            &delivery.node().0.to_string(),
            delivery.epoch,
            attempt_number,
        )?;
        let mut child = Command::new(&self.settings.python)
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
        self.active.add(delivery);
        reap(child);
        let name = root.file_name().unwrap().to_string_lossy();
        write(&self.settings.runs.join(format!(".ready-{name}")), delivery)?;
        Ok(Completion::confirmed(vec![]))
    }

    fn retire(&mut self, store: &mut Store) -> Result<()> {
        let snapshot = store.load()?;
        if snapshot.rows.iter().any(|row| row.table == "inbox") {
            return Ok(());
        }
        let world = crate::snapshot::decode(&snapshot)?;
        let mut retained = BTreeSet::new();
        for effect in &snapshot.pending_effects {
            let delivery: Delivery = serde_json::from_value(effect.payload.clone())?;
            retained.insert(delivery.node().0);
        }
        for row in snapshot.rows.iter().filter(|row| row.table == "watches") {
            let delivery: Delivery = serde_json::from_value(row.value.clone())?;
            retained.insert(delivery.node().0);
        }
        let sessions = self.settings.runs.join("sessions");
        let entries = match fs::read_dir(&sessions) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(node) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u64>().ok())
            else {
                continue;
            };
            if !retained.contains(&node)
                && world
                    .nodes
                    .get(node as usize)
                    .is_some_and(|n| n.phase.is_closed())
            {
                fs::remove_dir_all(entry.path())?;
                File::open(&sessions)?.sync_all()?;
            }
        }
        Ok(())
    }

    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        match delivery.output {
            Out::Effect {
                kind: EffectKind::StartRun(role),
                ..
            } if protocol::role(role).is_some() => {
                let result = inspect(&self.root(&delivery.key), delivery)?;
                if !matches!(result, Reconciliation::Pending) {
                    self.active.remove(delivery);
                }
                Ok(result)
            }
            Out::Effect {
                kind: EffectKind::StopRun,
                ..
            } => Fake::new(&self.fake_root).reconcile(delivery),
            _ => Fake::new(&self.fake_root).reconcile(delivery),
        }
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

pub(super) fn failed(delivery: &Delivery) -> Completion {
    Completion::confirmed(vec![Observation::Kernel(Event::Reply {
        node: delivery.node(),
        generation: delivery.generation,
        reply: Reply::RunFailed,
    })])
}

pub(super) fn inspect(root: &Path, delivery: &Delivery) -> Result<Reconciliation> {
    let Some((_, attempt)) = latest(root)? else {
        return Ok(Reconciliation::Absent);
    };
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
