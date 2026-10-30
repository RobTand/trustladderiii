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
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};
use tl3_kernel::{EffectKind, Event, Out, Reply};
use tl3_store::Store;

#[derive(Clone)]
pub struct Supervisor {
    settings: tl3_config::Workers,
    models: tl3_config::Models,
    store: tl3_config::Store,
    fake_root: PathBuf,
}

impl Supervisor {
    pub fn new(config: &tl3_config::Config, fake_root: &Path) -> Result<Self> {
        fs::create_dir_all(&config.workers.runs)?;
        Ok(Self {
            settings: config.workers.clone(),
            models: config.models.clone(),
            store: config.store.clone(),
            fake_root: fake_root.to_owned(),
        })
    }

    pub fn source(&self) -> WorkerOutputSource {
        WorkerOutputSource::new(self.settings.runs.clone(), self.store.clone())
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
        for entry in fs::read_dir(&self.settings.runs)? {
            let path = entry?.path();
            if let Some(value) = read(&path.join("delivery.json"))? {
                let run: Delivery = serde_json::from_value(value)?;
                if run.node() == delivery.node() {
                    self.stop_run(&run.key, delivery.epoch)?;
                }
            }
        }
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
                return Ok(Completion::confirmed(vec![]));
            }
            _ => return Err(Error::Invalid("unsupported worker effect")),
        };
        let Some(role) = role else {
            return Fake::new(&self.fake_root).execute(delivery);
        };
        match inspect(&self.root(&delivery.key), delivery)? {
            Reconciliation::Found(completion) => return Ok(completion),
            Reconciliation::Pending => return Ok(Completion::confirmed(vec![])),
            Reconciliation::Absent => {}
        }
        let root = self.root(&delivery.key);
        fs::create_dir_all(root.join("worktree"))?;
        let request = self.request(delivery, role, &mut store)?;
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
        let mut child = Command::new(&self.settings.uv)
            .args(["run", "--locked", "--project"])
            .arg(&self.settings.package)
            .args(["--python", "3.14+gil", "python"])
            .arg(attempt.join("supervisor.py"))
            .arg(&attempt)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(errors)
            .spawn()?;
        loop {
            if read(&attempt.join("claim.json"))?.is_some() {
                break;
            }
            if child.try_wait()?.is_some() {
                return Ok(failed(delivery));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        reap(child);
        Ok(Completion::confirmed(vec![]))
    }

    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.fence(delivery)?;
        match delivery.output {
            Out::Effect {
                kind: EffectKind::StartRun(role),
                ..
            } if protocol::role(role).is_some() => inspect(&self.root(&delivery.key), delivery),
            Out::Effect {
                kind: EffectKind::StopRun,
                ..
            } => {
                self.stop_node(delivery)?;
                Ok(Reconciliation::Found(Completion::confirmed(vec![])))
            }
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
