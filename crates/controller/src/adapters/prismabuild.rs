use super::record::Records;
use crate::{
    Result,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation},
};
use std::path::Path;
use tl3_kernel::{EffectKind, Out};
use tl3_kernel::{Event, Reply};

pub struct Fake(Records);
impl Fake {
    pub fn new(root: &Path) -> Self {
        Self(Records::new(root))
    }
}
impl Executor for Fake {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        let node = delivery.node();
        let observations = match delivery.output {
            Out::Effect {
                kind: EffectKind::SubmitChecks,
                ..
            } => vec![Observation::Kernel(Event::Reply {
                node,
                generation: delivery.generation,
                reply: Reply::ChecksDone {
                    report: true,
                    pass: true,
                    in_footprint: true,
                },
            })],
            Out::Effect {
                kind: EffectKind::WithdrawJob,
                ..
            } => vec![],
            _ => return Err(crate::Error::Invalid("unsupported PrismaBuild effect")),
        };
        self.0
            .deliver(delivery, Completion::confirmed(observations))
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.0.reconcile(delivery)
    }
}

mod checks;
mod results;
mod source;
pub use checks::{Check, CheckRun, Kind};
pub use source::{RunningSource, Source};

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, File},
    io::Write,
    path::PathBuf,
    time::Duration,
};
use tl3_prismabuild::{
    ActionKey, ActionState, Demand, PrismaBuild, ProgressPhase, SubmitAction, Tool,
};

#[derive(Clone, Debug)]
pub struct Settings {
    /// Private durable directory, owned by this controller's single adapter writer.
    pub records: PathBuf,
    /// Installed trusted package directory, outside every served checkout.
    pub harness: PathBuf,
    pub python: Vec<OsString>,
    pub image: String,
    pub demand: Demand,
    pub quiet: Duration,
    pub priority: i32,
    pub result_tool: Tool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Job {
    action: ActionKey,
    delivery: Delivery,
    run: CheckRun,
    changed_paths: Vec<String>,
}

pub struct Adapter {
    client: PrismaBuild,
    settings: Settings,
    runtime: tokio::runtime::Runtime,
    keys: tokio::sync::broadcast::Sender<String>,
}

fn adapter_error(error: tl3_prismabuild::Error) -> crate::Error {
    crate::Error::Adapter(Box::new(error))
}
fn encoded(key: &str) -> String {
    key.bytes().map(|b| format!("{b:02x}")).collect()
}
fn write(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = File::create(&temporary)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    File::open(
        path.parent()
            .ok_or(crate::Error::Invalid("record needs a parent"))?,
    )?
    .sync_all()?;
    Ok(())
}
fn read_job(path: &Path) -> Option<Job> {
    let result = fs::read(path)
        .map_err(crate::Error::from)
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(Into::into));
    match result {
        Ok(job) => Some(job),
        Err(error) => {
            eprintln!("PrismaBuild job {} skipped: {error}", path.display());
            None
        }
    }
}
fn job_paths(root: &Path, completed: bool) -> Result<Vec<PathBuf>> {
    let mut found = vec![];
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().is_some_and(|v| v == "job")
            && path.with_extension("done").exists() == completed
        {
            found.push(path);
        }
    }
    Ok(found)
}

fn retire_superseded(root: &Path, delivery: &Delivery) -> Result<()> {
    for path in job_paths(root, false)?
        .into_iter()
        .chain(job_paths(root, true)?)
    {
        let Some(job) = read_job(&path) else { continue };
        if job.delivery.node() == delivery.node() && job.delivery.generation < delivery.generation {
            // A later delivered check proves the prior effect and watch have settled.
            // Delete the binding first so a crash cannot expose a job without its done marker.
            fs::remove_file(&path)?;
            File::open(root)?.sync_all()?;
            match fs::remove_file(path.with_extension("done")) {
                Ok(()) => File::open(root)?.sync_all()?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

impl Adapter {
    pub fn new(client: PrismaBuild, settings: Settings) -> Result<Self> {
        if !settings.records.is_absolute()
            || !settings.harness.is_absolute()
            || settings.python.is_empty()
            || settings.quiet.is_zero()
            || settings.demand.gpu
        {
            return Err(crate::Error::Invalid(
                "invalid PrismaBuild adapter settings",
            ));
        }
        fs::create_dir_all(&settings.records)?;
        // No staging process survives the controller's exclusive store claim.
        for entry in fs::read_dir(&settings.records)? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "input") {
                fs::remove_dir_all(path)?;
                File::open(&settings.records)?.sync_all()?;
            } else if path.extension().is_some_and(|ext| ext == "done")
                && !path.with_extension("job").exists()
            {
                // Retirement may have crashed after durably deleting the binding.
                fs::remove_file(path)?;
                File::open(&settings.records)?.sync_all()?;
            }
        }
        Ok(Self {
            client,
            settings,
            keys: tokio::sync::broadcast::channel(64).0,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
        })
    }

    pub fn source(&self) -> Source {
        Source::new(
            self.client.clone(),
            self.settings.clone(),
            self.keys.subscribe(),
        )
    }

    pub fn action_key(&self, effect_key: &str) -> Result<Option<ActionKey>> {
        Ok(self.job(effect_key)?.map(|job| job.action))
    }

    fn job(&self, key: &str) -> Result<Option<Job>> {
        match fs::read(self.settings.records.join(format!("{}.job", encoded(key)))) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn fence(&self, epoch: i64) -> Result<()> {
        let path = self.settings.records.join("epoch.fence");
        let accepted = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e.into()),
        };
        if epoch < accepted {
            return Err(crate::Error::Invalid(
                "PrismaBuild adapter refused an older epoch",
            ));
        }
        write(&path, &epoch)
    }

    fn submit(&self, delivery: &Delivery) -> Result<Job> {
        if let Some(job) = self.job(&delivery.key)? {
            self.remove_input(&delivery.key)?;
            return Ok(job);
        }
        let input = self
            .settings
            .records
            .join(format!("{}.input", encoded(&delivery.key)));
        let changed_paths = checks::stage(
            &input,
            &self.settings.harness,
            delivery,
            &self.settings.image,
        )?;
        let mut command = self.settings.python.clone();
        command.extend(["-I".into(), "-B".into(), "action.py".into()]);
        let action = SubmitAction {
            checkout: input,
            command,
            demand: self.settings.demand.clone(),
            container_image: self.settings.image.clone(),
            progress: vec![ProgressPhase {
                name: "tests".into(),
                stall_allowance: self.settings.quiet,
            }],
            progress_cycle: false,
            env: BTreeMap::new(),
        };
        let key = self
            .runtime
            .block_on(
                self.client
                    .seal_at_priority(&action, self.settings.priority),
            )
            .map_err(adapter_error)?;
        let job = Job {
            action: key,
            delivery: delivery.clone(),
            changed_paths,
            run: delivery
                .checks
                .clone()
                .ok_or(crate::Error::Invalid("missing check plan"))?,
        };
        // The sealed key reaches stable storage before PB may execute anything.
        write(
            &self
                .settings
                .records
                .join(format!("{}.job", encoded(&delivery.key))),
            &job,
        )?;
        self.remove_input(&delivery.key)?;
        Ok(job)
    }

    fn remove_input(&self, key: &str) -> Result<()> {
        let input = self
            .settings
            .records
            .join(format!("{}.input", encoded(key)));
        match fs::remove_dir_all(input) {
            Ok(()) => File::open(&self.settings.records)?.sync_all()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    fn targets(&self, delivery: &Delivery) -> Result<Vec<Job>> {
        let mut targets = vec![];
        for path in job_paths(&self.settings.records, false)? {
            // An unreadable active binding cannot prove that withdrawal has finished.
            let job: Job = serde_json::from_slice(&fs::read(path)?)?;
            if job.delivery.node() == delivery.node()
                && Some(job.delivery.generation) == delivery.job_generation
            {
                targets.push(job);
            }
        }
        Ok(targets)
    }

    fn check_result(&self, job: &Job) -> Result<Reconciliation> {
        let done = self
            .settings
            .records
            .join(format!("{}.done", encoded(&job.delivery.key)));
        match fs::read(done) {
            Ok(bytes) => return Ok(Reconciliation::Found(serde_json::from_slice(&bytes)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let record = self
            .runtime
            .block_on(self.client.read(&job.action))
            .map_err(adapter_error)?;
        let Some(record) = record else {
            return Ok(Reconciliation::Absent);
        };
        if matches!(record.state, ActionState::Queued | ActionState::Claimed) {
            return Ok(Reconciliation::Pending);
        }
        Ok(
            match self
                .runtime
                .block_on(results::ending(&self.settings.result_tool, job, false))?
            {
                Some(completion) => Reconciliation::Found(completion),
                None => Reconciliation::Pending,
            },
        )
    }
}

impl Executor for Adapter {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.fence(delivery.epoch)?;
        match delivery.output {
            Out::Effect {
                kind: EffectKind::SubmitChecks,
                ..
            } => {
                let job = self.submit(delivery)?;
                let _ = self.keys.send(delivery.key.clone());
                if self
                    .runtime
                    .block_on(self.client.read(&job.action))
                    .map_err(adapter_error)?
                    .is_none()
                {
                    self.runtime
                        .block_on(self.client.publish(&job.action))
                        .map_err(adapter_error)?;
                }
                Ok(Completion::confirmed(vec![]))
            }
            Out::Effect {
                kind: EffectKind::WithdrawJob,
                ..
            } => {
                self.remove_input(&delivery.key)?;
                let targets = self.targets(delivery)?;
                for job in &targets {
                    self.remove_input(&job.delivery.key)?;
                    self.runtime
                        .block_on(self.client.withdraw(&job.action, "controller withdrawal"))
                        .map_err(adapter_error)?;
                }
                match self.reconcile(delivery)? {
                    Reconciliation::Found(completion) => Ok(completion),
                    _ => Err(crate::Error::Invalid(
                        "PrismaBuild withdrawal still awaits its ending",
                    )),
                }
            }
            _ => Err(crate::Error::Invalid("unsupported PrismaBuild effect")),
        }
    }

    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        match delivery.output {
            Out::Effect {
                kind: EffectKind::SubmitChecks,
                ..
            } => {
                // Reconciliation also settles cancelled effects that will never execute again.
                self.remove_input(&delivery.key)?;
                let Some(job) = self.job(&delivery.key)? else {
                    // Publication requires this durable binding, so no attempt can exist without it.
                    return Ok(Reconciliation::Absent);
                };
                self.check_result(&job)
            }
            Out::Effect {
                kind: EffectKind::WithdrawJob,
                ..
            } => {
                for job in self.targets(delivery)? {
                    self.remove_input(&job.delivery.key)?;
                    let Some(record) = self
                        .runtime
                        .block_on(self.client.read(&job.action))
                        .map_err(adapter_error)?
                    else {
                        // Authoritative absence also ends a source watch for a cancelled seal.
                        write(
                            &self
                                .settings
                                .records
                                .join(format!("{}.done", encoded(&job.delivery.key))),
                            &Completion::confirmed(vec![]),
                        )?;
                        let _ = self.keys.send(job.delivery.key.clone());
                        continue;
                    };
                    if matches!(record.state, ActionState::Queued | ActionState::Claimed) {
                        self.fence(delivery.epoch)?;
                        self.runtime
                            .block_on(self.client.withdraw(&job.action, "controller withdrawal"))
                            .map_err(adapter_error)?;
                    }
                    if self
                        .runtime
                        .block_on(results::ending(&self.settings.result_tool, &job, true))?
                        .is_none()
                    {
                        return Ok(Reconciliation::Pending);
                    }
                }
                Ok(Reconciliation::Found(Completion::confirmed(vec![])))
            }
            _ => Err(crate::Error::Invalid("unsupported PrismaBuild effect")),
        }
    }
}
