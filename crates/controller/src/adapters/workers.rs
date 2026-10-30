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
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
};
use tl3_kernel::{EffectKind, Event, Out, Reply};
use tl3_store::Store;

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
                Ok(Reconciliation::Pending) => active.add(&delivery),
                Ok(_) => {}
                Err(error) => eprintln!("worker startup skipped {}: {error}", delivery.key),
            }
        }
        Ok(Self {
            active,
            settings: config.workers.clone(),
            models: config.models.clone(),
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

    fn fence(&self, delivery: &Delivery) -> Result<Store> {
        let mut store = Store::open(&self.store)?;
        if store.load()?.epoch.get() != delivery.epoch {
            return Err(Error::Invalid("worker epoch is fenced"));
        }
        Ok(store)
    }

    fn root(&self, key: &str) -> PathBuf {
        run_root(&self.settings.runs, key)
    }

    fn request(&self, delivery: &Delivery, role: &str, store: &mut Store) -> Result<Value> {
        let node = delivery.node().0.to_string();
        let mut inputs = store
            .worker_materials(&node)?
            .ok_or(Error::Invalid("worker materials are missing"))?;
        if role == "audit" {
            inputs["ingest"] = store
                .worker_document(&node, "briefs")?
                .ok_or(Error::Invalid("audit needs an ingest reply"))?;
        } else if let Some(audit) = store.worker_document(&node, "audits")?
            && audit["verdict"] == "disagree"
        {
            inputs["audit_objections"] = audit["objections"].clone();
        }
        protocol::validate_inputs(role, &inputs)?;
        let ladder = if role == "ingest" {
            &self.models.ingest
        } else {
            &self.models.audit
        };
        let model = &ladder[usize::from(delivery.attempt.saturating_sub(1)).min(ladder.len() - 1)];
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
        let mut store = self.fence(delivery)?;
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
        let request = self.request(delivery, role, &mut store)?;
        let attempt_number = previous.map_or(0, |(n, _)| n + 1);
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

    fn retire(&mut self, store: &mut Store) -> Result<()> {
        for key in store.retired_workers()? {
            let root = self.root(&key);
            if let Some((_, attempt)) = latest(&root)? {
                cancel(&attempt.join("claim.json"))?;
                cancel(&attempt.join("process.json"))?;
                process::kill_and_wait(&attempt.join("process.json"))?;
                process::kill_and_wait(&attempt.join("claim.json"))?;
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
            store.forget_worker(&key)?;
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
        Ok(bytes) => source::parse_reply(&bytes, delivery),
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
