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
    executor::{Completion, Delivery, Executor, Look, Observation, Reconciliation},
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
use tl3_kernel::{EffectKind, Event, Out, Reply};
use tl3_store::{Snapshot, Store};

#[derive(Clone, Default)]
pub(super) struct ActiveRuns(Arc<Mutex<BTreeMap<String, BTreeMap<String, i64>>>>);
impl ActiveRuns {
    fn add(&self, delivery: &Delivery, attempt: i64) {
        self.0
            .lock()
            .unwrap()
            .entry(delivery.node().0.to_string())
            .or_default()
            .entry(delivery.key.clone())
            .and_modify(|current| *current = (*current).max(attempt))
            .or_insert(attempt);
    }
    fn remove(&self, delivery: &Delivery, attempt: i64) {
        let mut nodes = self.0.lock().unwrap();
        let node = delivery.node().0.to_string();
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
    fake_root: PathBuf,
    active: ActiveRuns,
}

impl Supervisor {
    pub fn new(config: &tl3_config::Config, fake_root: &Path) -> Result<Self> {
        fs::create_dir_all(config.workers.runs.join(".ready"))?;
        fs::create_dir_all(fake_root)?;
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
            fake_root: fake_root.to_owned(),
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
        let mut inputs = store
            .worker_materials(&node)?
            .ok_or(Error::Invalid("worker materials are missing"))?;
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
        } else if let Some(audit) = store.worker_document(&node, "audits")?
            && audit["verdict"] == "disagree"
        {
            inputs["audit_objections"] = audit["objections"].clone();
        }
        if role == "ingest" {
            inputs["forecast"] = forecast;
        }
        protocol::validate_inputs(role, &inputs)?;
        let ladder = if role == "ingest" {
            &self.models.ingest
        } else if role == "audit" {
            &self.models.audit
        } else {
            &self.models.planner
        };
        let model = &ladder[rung.min(ladder.len() - 1)];
        Ok(json!({
            "protocol": 1,
            "operation": {"node": delivery.node().0, "generation": delivery.generation, "kind": role},
            "role": role, "model": self.models.worker_model(model), "key_file": self.models.key_file,
            "inputs": inputs, "worktree": self.root(&delivery.key).join("worktree"),
        }))
    }

    /// Stop and prove termination before acknowledging teardown or a stall.
    pub fn stop_run(&self, key: &str, epoch: i64) -> Result<()> {
        if Store::open(&self.store)?.load()?.epoch.get() != epoch {
            return Err(Error::Invalid("worker stop epoch is fenced"));
        }
        stop_attempt(&self.root(key))?;
        Ok(())
    }

    fn retire_run(&self, key: &str, store: &mut Store) -> Result<()> {
        let root = self.root(key);
        stop_attempt(&root)?;
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
        let request = self.request(delivery, role, &mut store, &snapshot)?;
        let attempt_number = previous.map_or(0, |(n, _)| n + 1);
        store.claim_worker(
            &delivery.key,
            &delivery.node().0.to_string(),
            delivery.epoch,
            attempt_number,
        )?;
        fs::create_dir_all(root.join("worktree"))?;
        if role != "audit" {
            let materials = store
                .worker_materials(&delivery.node().0.to_string())?
                .ok_or(Error::Invalid("worker materials missing"))?;
            materialize(&root.join("worktree"), &materials["repo"])?;
        }
        let attempt = root.join(format!("attempt-{attempt_number:08}"));
        fs::create_dir(&attempt)?;
        write(&attempt.join("request.json"), &request)?;
        write(&attempt.join("command.json"), &self.settings.command)?;
        let mut script = File::create(attempt.join("supervisor.py"))?;
        script.write_all(include_bytes!("../../tools/worker_supervisor.py"))?;
        script.sync_all()?;
        File::open(&attempt)?.sync_all()?;
        File::open(&root)?.sync_all()?;
        let errors = File::create(attempt.join("launcher.log"))?;
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
                    Some(row) => Look::Alive {
                        progress: format!("{}:{}", row.attempt, row.output_offset),
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
                let result = match latest(&self.root(&delivery.key))? {
                    Some((_, attempt)) => inspect(&attempt, delivery)?,
                    None => Reconciliation::Absent,
                };
                if !matches!(result, Reconciliation::Pending) {
                    if let Some((number, _)) = latest(&self.root(&delivery.key))? {
                        self.active.remove(delivery, number);
                    }
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
                if matches!(delivery.output, Out::Effect { kind: EffectKind::StartRun(role), .. } if protocol::role(role).is_some())
                {
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
