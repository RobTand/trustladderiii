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
    ActionKey, ActionState, Demand, PrismaBuild, ProgressPhase, SubmissionStatus, SubmitAction,
    Tool,
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
fn jobs(root: &Path) -> Result<Vec<Job>> {
    let mut found = vec![];
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().is_some_and(|v| v == "job") {
            found.push(serde_json::from_slice(&fs::read(path)?)?);
        }
    }
    Ok(found)
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
        Ok(Self {
            client,
            settings,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
        })
    }

    pub fn source(&self) -> Source {
        Source::new(self.client.clone(), self.settings.clone())
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
        Ok(job)
    }

    fn targets(&self, delivery: &Delivery) -> Result<Vec<Job>> {
        Ok(jobs(&self.settings.records)?
            .into_iter()
            .filter(|job| {
                job.delivery.node() == delivery.node()
                    && Some(job.delivery.generation) == delivery.job_generation
            })
            .collect())
    }

    fn check_result(&self, job: &Job) -> Result<Reconciliation> {
        let record = self
            .runtime
            .block_on(self.client.read(&job.action))
            .map_err(adapter_error)?;
        if record.is_some_and(|r| matches!(r.state, ActionState::Queued | ActionState::Claimed)) {
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
                let targets = self.targets(delivery)?;
                for job in &targets {
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
                let job = match self.job(&delivery.key)? {
                    Some(job) => job,
                    None => {
                        // No runnable work can exist before this binding. Reassert the same effect;
                        // unavailable input stays uncertain instead of spinning through Absent.
                        self.fence(delivery.epoch)?;
                        self.submit(delivery)?
                    }
                };
                if self
                    .runtime
                    .block_on(self.client.read(&job.action))
                    .map_err(adapter_error)?
                    .is_none()
                {
                    // Reassert only the original immutable request. An older epoch may read, never publish.
                    self.fence(delivery.epoch)?;
                    let submission = self
                        .runtime
                        .block_on(self.client.publish(&job.action))
                        .map_err(adapter_error)?;
                    if submission.status != SubmissionStatus::CacheHit {
                        return Ok(Reconciliation::Found(Completion::confirmed(vec![])));
                    }
                }
                self.check_result(&job)
            }
            Out::Effect {
                kind: EffectKind::WithdrawJob,
                ..
            } => {
                for job in self.targets(delivery)? {
                    let Some(record) = self
                        .runtime
                        .block_on(self.client.read(&job.action))
                        .map_err(adapter_error)?
                    else {
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
