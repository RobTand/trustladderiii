//! Controller delivery journal and check executor over the shared execution transport.
use super::{
    backend_watch,
    prismabuild::{CheckRun, results},
};
use crate::run_backend::{self as backend, durable};
use crate::{
    Error, Result,
    cli::Input,
    executor::{Completion, Delivery, Executor, Output, Reconciliation},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::Arc,
};
use tl3_kernel::EffectKind;

/// Trusted staging supplies shared runtime types; it never launches native work.
pub trait Preparation {
    fn prepare(&mut self, delivery: &Delivery) -> Result<PreparedCheck>;
    fn capacity(&self, delivery: &Delivery) -> Result<(backend::Reservation, backend::Placement)>;
}

/// Worker-owned wait routing composes with daytime execution through the same backend.
/// Implementations must bind subscriptions to the exact worker attempt and checkout.
pub trait WorkerWaits {
    fn watch_wait(
        &mut self,
        delivery: &Delivery,
        wait: Option<&tl3_store::ExternalWait>,
    ) -> Result<()>;
    fn wait_record(&mut self, key: &str, checkout: &Path) -> Result<crate::executor::WaitRecord>;
}

pub struct PreparedCheck {
    pub request: backend::Request,
    pub run: CheckRun,
    pub changed_paths: Vec<String>,
    pub selected_tests: Option<u32>,
    pub always_tests: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    delivery: Delivery,
    request: backend::Request,
    intent: backend::SubmissionIntent,
    run: CheckRun,
    changed_paths: Vec<String>,
    selected_tests: Option<u32>,
    always_tests: Vec<String>,
    binding: Option<backend::RunBinding>,
    terminal: Option<backend::Terminal>,
    completion: Option<Completion>,
    #[serde(default)]
    progress: Option<backend::NativeProgress>,
}
impl Job {
    fn validate(&self, key: &str) -> Result<()> {
        if key != correlation(&self.delivery)
            || self.intent.owner != "controller-checks"
            || self.intent.correlation != key
            || self.intent.request_digest != durable::digest(&serde_json::to_vec(&self.request)?)
            || self.intent.output_dir != Path::new("/mnt/shared/flux-out/tl3").join(key)
            || self.completion.is_some() && self.terminal.is_none()
        {
            return Err(Error::Invalid("invalid durable controller check intent"));
        }
        if let Some(binding) = &self.binding {
            binding
                .id
                .validate()
                .map_err(|error| adapter_error(error.into()))?;
            if binding.intent != self.intent
                || binding.id.instance != self.intent.instance
                || binding.id.uri != self.intent.uri
            {
                return Err(Error::Invalid(
                    "controller check binding does not match retained intent",
                ));
            }
        }
        Ok(())
    }
    fn owner(&self) -> backend::Owner {
        backend::Owner {
            owner: self.intent.owner.clone(),
            correlation: self.intent.correlation.clone(),
        }
    }
    fn context(&self) -> results::ReportContext<'_> {
        results::ReportContext {
            delivery: &self.delivery,
            run: &self.run,
            changed_paths: &self.changed_paths,
            selected_tests: self.selected_tests,
            always_tests: &self.always_tests,
        }
    }
    fn matches(&self, delivery: &Delivery) -> Result<bool> {
        Ok(self.delivery.key == delivery.key
            && self.delivery.generation == delivery.generation
            && self.delivery.job_generation == delivery.job_generation
            && self.delivery.head_sha == delivery.head_sha
            && self.delivery.repository == delivery.repository
            && self.delivery.issue == delivery.issue
            && self.delivery.pull_request == delivery.pull_request
            && self.delivery.round == delivery.round
            && self.delivery.paths == delivery.paths
            && (delivery.checks == self.delivery.checks
                || delivery.checks.as_ref() == Some(&self.run))
            && serde_json::to_value(self.delivery.output)?
                == serde_json::to_value(delivery.output)?)
    }
}

fn adapter_error(error: backend::BackendError) -> Error {
    Error::Adapter(Box::new(error))
}
fn correlation(delivery: &Delivery) -> String {
    durable::digest(delivery.key.as_bytes())
}

struct Shared {
    backend: backend_watch::Backend,
    options: backend::Options,
    root: PathBuf,
}
impl Shared {
    // Cross-process serialization covers only controller journal I/O, never native attach/cancel.
    fn transaction<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let path = self.root.join("lock");
        if path.is_symlink() {
            return Err(Error::Invalid("check journal lock cannot be a symlink"));
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        lock.lock()?;
        f()
    }
    fn path(&self, key: &str) -> Result<PathBuf> {
        if !backend::hex(key, 64) {
            return Err(Error::Invalid("invalid controller check correlation"));
        }
        Ok(self.root.join(format!("{key}.job")))
    }
    fn load(&self, key: &str) -> Result<Option<Job>> {
        let path = self.path(key)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                let job: Job =
                    serde_json::from_slice(&durable::read(&path).map_err(adapter_error)?)?;
                job.validate(key)?;
                Ok(Some(job))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    fn save(&self, job: &Job) -> Result<()> {
        let bytes = serde_json::to_vec(job)?;
        if bytes.len() > durable::MAX_BYTES {
            return Err(Error::Invalid("oversized controller check intent"));
        }
        durable::atomic(&self.path(&job.intent.correlation)?, &bytes).map_err(adapter_error)
    }
    fn jobs(&self) -> Result<Vec<Job>> {
        let mut jobs = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.extension().is_some_and(|extension| extension == "job") {
                let key = path
                    .file_stem()
                    .and_then(|key| key.to_str())
                    .ok_or(Error::Invalid("invalid check journal name"))?;
                jobs.push(
                    self.load(key)?
                        .ok_or(Error::Invalid("check journal disappeared"))?,
                );
            }
        }
        Ok(jobs)
    }
    fn fence(&self, incoming: i64) -> Result<()> {
        let path = self.root.join("epoch.fence");
        let accepted: i64 = match fs::symlink_metadata(&path) {
            Ok(_) => serde_json::from_slice(&durable::read(&path).map_err(adapter_error)?)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error.into()),
        };
        if incoming < accepted || incoming < 0 {
            return Err(Error::Invalid("check adapter refused an older epoch"));
        }
        durable::atomic(&path, &serde_json::to_vec(&incoming)?).map_err(adapter_error)
    }
    fn progress(&self, owner: &backend::Owner, event: &backend::Event) -> Result<Option<Input>> {
        self.transaction(|| {
            let mut job = self
                .load(&owner.correlation)?
                .ok_or(Error::Invalid("progress has no controller intent"))?;
            if job.owner() != *owner
                || event.progress.owner != *owner
                || event.progress.run.instance != job.intent.instance
                || event.progress.run.uri != job.intent.uri
                || event.progress.cursor.sequence != event.sequence
                || event.progress.cursor.digest != event.digest
                || event.progress.authority != backend::ProgressAuthority::Historical
                || job
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.id != event.progress.run)
            {
                return Err(Error::Invalid(
                    "typed progress does not match the controller check binding",
                ));
            }
            event
                .progress
                .run
                .validate()
                .map_err(|error| adapter_error(error.into()))?;
            if let Some(previous) = &job.progress {
                if previous.cursor.sequence > event.sequence {
                    return Ok(None);
                }
                if previous.cursor.sequence == event.sequence {
                    if previous != &event.progress {
                        return Err(Error::Invalid("conflicting check progress cursor"));
                    }
                    return Ok(None);
                }
            }
            job.progress = Some(event.progress.clone());
            self.save(&job)?;
            Ok(Some(Input::Observation {
                key: format!(
                    "flux-progress:{}:{}:{}",
                    owner.correlation, event.sequence, event.digest
                ),
                observation: crate::executor::Observation::Progress {
                    key: job.delivery.key,
                    checkpoint: format!("flux:{}:{}", event.sequence, event.digest),
                },
            }))
        })
    }
    fn observe(
        &self,
        owner: &backend::Owner,
        observed: backend::Observation,
    ) -> Result<Option<Input>> {
        let job = self
            .transaction(|| self.load(&owner.correlation))?
            .ok_or(Error::Invalid("observer has no durable controller intent"))?;
        if job.owner() != *owner {
            return Err(Error::Invalid("check observer owner mismatch"));
        }
        let (binding, terminal) = match observed {
            backend::Observation::Unknown { binding, .. } => (binding, None),
            backend::Observation::Terminal { binding, terminal } => (binding, Some(terminal)),
        };
        if let Some(binding) = &binding {
            binding
                .id
                .validate()
                .map_err(|error| adapter_error(error.into()))?;
        }
        if binding.as_ref().is_some_and(|binding| {
            binding.intent != job.intent
                || binding.id.instance != job.intent.instance
                || binding.id.uri != job.intent.uri
        }) {
            return Err(Error::Invalid(
                "check observation belongs to another submission",
            ));
        }
        let completion = if let Some(terminal) = &terminal {
            match terminal.ending {
                backend::Ending::Passed => Some(results::daytime_completion(
                    self.backend.as_ref(),
                    owner,
                    terminal,
                    &job.context(),
                    &format!("flux:{}", owner.correlation),
                )?),
                // An infrastructure terminal is never a semantic rejection of a member's checks.
                _ => Some(results::transport_completion(
                    &job.context(),
                    &format!("flux:{}", owner.correlation),
                )?),
            }
        } else {
            None
        };
        self.transaction(|| {
            let mut latest = self
                .load(&owner.correlation)?
                .ok_or(Error::Invalid("controller check intent disappeared"))?;
            if let Some(binding) = binding {
                if latest
                    .binding
                    .as_ref()
                    .is_some_and(|saved| saved != &binding)
                {
                    return Err(Error::Invalid("controller check binding conflict"));
                }
                latest.binding = Some(binding);
            }
            if let Some(terminal) = terminal {
                if latest
                    .terminal
                    .as_ref()
                    .is_some_and(|saved| saved != &terminal)
                {
                    return Err(Error::Invalid("controller check terminal conflict"));
                }
                latest.terminal = Some(terminal);
            }
            if latest.completion.is_none() {
                latest.completion = completion;
            }
            self.save(&latest)?; // Journal before ingress, including a replay after receiver loss.
            Ok(latest.completion.map(|completion| Input::Completion {
                key: latest.delivery.key,
                completion: Box::new(completion),
            }))
        })
    }
}

pub struct Adapter {
    shared: Arc<Shared>,
    preparation: Box<dyn Preparation>,
    worker_waits: Option<Box<dyn WorkerWaits>>,
    handle: backend_watch::Handle,
    source: Option<backend_watch::Source>,
}
impl Adapter {
    pub fn new(
        backend: backend_watch::Backend,
        options: backend::Options,
        root: &Path,
        preparation: impl Preparation + 'static,
        ownership: &mut crate::Ownership,
    ) -> Result<Self> {
        ownership.ensure_held()?;
        if !backend::absolute(root) || root.is_symlink() {
            return Err(Error::Invalid(
                "check journal requires a private absolute directory",
            ));
        }
        fs::create_dir_all(root)?;
        if let Some(parent) = root.parent() {
            File::open(parent)?.sync_all()?;
        }
        let shared = Arc::new(Shared {
            backend,
            options,
            root: root.to_owned(),
        });
        let recovered = shared
            .transaction(|| Ok(shared.jobs()?.into_iter().map(|job| job.owner()).collect()))?;
        let finishing = shared.clone();
        let progressing = shared.clone();
        let (handle, source) = backend_watch::Source::new(
            shared.backend.clone(),
            recovered,
            Arc::new(move |owner, observation| finishing.observe(owner, observation)),
        );
        let source = source.with_progress(Arc::new(move |owner, event| {
            progressing.progress(owner, event)
        }));
        Ok(Self {
            shared,
            preparation: Box::new(preparation),
            worker_waits: None,
            handle,
            source: Some(source),
        })
    }
    pub fn with_worker_waits(mut self, waits: impl WorkerWaits + 'static) -> Self {
        self.worker_waits = Some(Box::new(waits));
        self
    }
    pub fn source(&mut self) -> Result<backend_watch::Source> {
        self.source
            .take()
            .ok_or(Error::Invalid("check source already taken"))
    }
    fn targets(&self, delivery: &Delivery) -> Result<Vec<Job>> {
        let generation = delivery.job_generation.ok_or(Error::Invalid(
            "withdrawal requires an exact job generation",
        ))?;
        self.shared.transaction(|| {
            Ok(self
                .shared
                .jobs()?
                .into_iter()
                .filter(|job| {
                    job.delivery.node() == delivery.node() && job.delivery.generation == generation
                })
                .collect())
        })
    }
}
impl Executor for Adapter {
    fn watch_wait(
        &mut self,
        delivery: &Delivery,
        wait: Option<&tl3_store::ExternalWait>,
    ) -> Result<()> {
        match &mut self.worker_waits {
            Some(waits) => waits.watch_wait(delivery, wait),
            None if wait.is_none() => Ok(()),
            None => Err(Error::Invalid("Flux worker wait routing is not registered")),
        }
    }
    fn wait_record(&mut self, key: &str, checkout: &Path) -> Result<crate::executor::WaitRecord> {
        self.worker_waits
            .as_mut()
            .ok_or(Error::Invalid("Flux worker wait routing is not registered"))?
            .wait_record(key, checkout)
    }
    fn contract_evidence(
        &mut self,
        delivery: &Delivery,
    ) -> Result<crate::dispatch_liveness::decision::ContractEvidence> {
        use crate::dispatch_liveness::decision::ContractEvidence;
        Ok(
            match self
                .shared
                .transaction(|| self.shared.load(&correlation(delivery)))?
            {
                Some(job) if job.matches(delivery)? && job.completion.is_some() => {
                    ContractEvidence::Ending
                }
                Some(job) if job.matches(delivery)? && job.binding.is_some() => {
                    ContractEvidence::ActionBound
                }
                _ => ContractEvidence::Absent,
            },
        )
    }
    fn probe(&mut self, delivery: &Delivery) -> Result<bool> {
        let (reservation, placement) = match self
            .shared
            .transaction(|| self.shared.load(&correlation(delivery)))?
        {
            Some(job) => (job.request.reservation, job.request.runtime.placement()),
            None => self.preparation.capacity(delivery)?,
        };
        Ok(self
            .shared
            .backend
            .probe(&reservation, &placement)
            .is_ok_and(|probe| {
                matches!(
                    probe.availability,
                    backend::Availability::Available | backend::Availability::Busy
                )
            }))
    }
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.shared
            .transaction(|| self.shared.fence(delivery.epoch))?;
        match delivery.output {
            Output::Effect {
                kind: EffectKind::SubmitChecks,
                ..
            } => {
                let existing = self
                    .shared
                    .transaction(|| self.shared.load(&correlation(delivery)))?;
                if let Some(job) = existing {
                    if !job.matches(delivery)? {
                        return Err(Error::Invalid(
                            "check effect changed its immutable identity",
                        ));
                    }
                    self.handle.watch(job.owner())?;
                    return job
                        .completion
                        .or_else(|| job.binding.map(|_| Completion::confirmed(vec![])))
                        .ok_or(Error::Invalid(
                            "check acceptance remains unknown; reattach without resubmission",
                        ));
                }
                if delivery.checks.is_none() {
                    return Err(Error::ChecksPending);
                }
                let prepared = self.preparation.prepare(delivery)?;
                match &prepared.request.suite {
                    backend::Suite::DaytimeChecks {
                        base,
                        head,
                        reviewed,
                        ..
                    } if base == prepared.run.base_sha.as_str()
                        && Some(head.as_str()) == delivery.head_sha.as_deref()
                        && reviewed.as_deref()
                            == prepared.run.reviewed_sha.as_ref().map(|sha| sha.as_str()) => {}
                    _ => {
                        return Err(Error::Invalid(
                            "controller checks require the exact served base/head/reviewed daytime suite",
                        ));
                    }
                }
                let key = correlation(delivery);
                let intent = backend::SubmissionIntent {
                    backend: backend::BackendTag::Flux,
                    instance: self.shared.options.instance_generation.clone(),
                    uri: self.shared.options.uri.clone(),
                    owner: "controller-checks".into(),
                    correlation: key.clone(),
                    request_digest: durable::digest(&serde_json::to_vec(&prepared.request)?),
                    output_dir: Path::new("/mnt/shared/flux-out/tl3").join(&key),
                };
                let job = Job {
                    delivery: delivery.clone(),
                    request: prepared.request,
                    intent,
                    run: prepared.run,
                    changed_paths: prepared.changed_paths,
                    selected_tests: prepared.selected_tests,
                    always_tests: prepared.always_tests,
                    binding: None,
                    terminal: None,
                    completion: None,
                    progress: None,
                };
                self.shared.transaction(|| {
                    self.shared.fence(delivery.epoch)?;
                    if self.shared.load(&key)?.is_some() {
                        return Err(Error::Invalid(
                            "check intent was concurrently retained; reconcile before dispatch",
                        ));
                    }
                    self.shared.save(&job)
                })?;
                let submission = self
                    .shared
                    .backend
                    .submit(job.request.clone(), job.intent.clone());
                // Recover even if a transport error follows native acceptance.
                self.handle.watch(job.owner())?;
                match submission.map_err(adapter_error)? {
                    backend::Submission::Accepted { binding } => {
                        self.shared.observe(
                            &job.owner(),
                            backend::Observation::Unknown {
                                binding: Some(binding),
                                reason: "accepted".into(),
                            },
                        )?;
                        Ok(Completion::confirmed(vec![]))
                    }
                    backend::Submission::Refused { terminal } => {
                        self.shared.observe(
                            &job.owner(),
                            backend::Observation::Terminal {
                                binding: None,
                                terminal,
                            },
                        )?;
                        Err(Error::Invalid(
                            "check submission refused before semantic evidence",
                        ))
                    }
                    backend::Submission::Unknown { .. } => Err(Error::Invalid(
                        "check acceptance remains unknown; retained for attach",
                    )),
                }
            }
            Output::Effect {
                kind: EffectKind::WithdrawJob,
                ..
            } => {
                for job in self.targets(delivery)? {
                    if job.terminal.is_none() {
                        self.shared
                            .backend
                            .cancel(&job.owner())
                            .map_err(adapter_error)?;
                        self.handle.watch(job.owner())?;
                    }
                }
                match self.reconcile(delivery)? {
                    Reconciliation::Found(completion) => Ok(*completion),
                    _ => Err(Error::Invalid(
                        "withdrawal awaits terminal evidence, not cancellation acknowledgement",
                    )),
                }
            }
            _ => Err(Error::Invalid("unsupported shared check effect")),
        }
    }
    fn look(&mut self, delivery: &Delivery) -> Result<crate::executor::Look> {
        use crate::executor::Look;
        if !matches!(
            delivery.output,
            Output::Effect {
                kind: EffectKind::SubmitChecks,
                ..
            }
        ) {
            return Ok(match self.reconcile(delivery)? {
                Reconciliation::Found(completion) => Look::Found(completion),
                Reconciliation::Absent => Look::Absent,
                Reconciliation::Pending => Look::Pending,
            });
        }
        let Some(job) = self
            .shared
            .transaction(|| self.shared.load(&correlation(delivery)))?
        else {
            return Ok(Look::Absent);
        };
        if !job.matches(delivery)? {
            return Err(Error::Invalid("check look identity changed"));
        }
        if let Some(completion) = &job.completion {
            return Ok(Look::Found(Box::new(completion.clone())));
        }
        self.handle.watch(job.owner())?;
        let live = self
            .shared
            .backend
            .liveness(&job.owner())
            .map_err(adapter_error)?;
        if live.owner != job.owner() {
            return Err(Error::Invalid("native liveness belongs to another owner"));
        }
        let binding = live.run.map(|id| backend::RunBinding {
            id,
            intent: job.intent.clone(),
        });
        self.shared.observe(
            &job.owner(),
            backend::Observation::Unknown {
                binding: binding.clone(),
                reason: "bounded native liveness".into(),
            },
        )?;
        let current = self
            .shared
            .transaction(|| self.shared.load(&correlation(delivery)))?
            .ok_or(Error::Invalid("check intent disappeared"))?;
        if let Some(completion) = current.completion {
            return Ok(Look::Found(Box::new(completion)));
        }
        if live.authority == backend::ProgressAuthority::CommittedTerminal
            && live.phase == backend::NativePhase::Terminal
        {
            let terminal = live
                .terminal
                .ok_or(Error::Invalid("terminal liveness has no committed ending"))?;
            self.shared.observe(
                &job.owner(),
                backend::Observation::Terminal { binding, terminal },
            )?;
            return Ok(match self.reconcile(delivery)? {
                Reconciliation::Found(completion) => Look::Found(completion),
                _ => Look::Pending,
            });
        }
        if live.authority == backend::ProgressAuthority::FreshNative
            && live.evidence.is_some()
            && current.binding.is_some()
            && live.observed_unix_ms > 0
            && matches!(
                live.phase,
                backend::NativePhase::Queued
                    | backend::NativePhase::Running
                    | backend::NativePhase::Finishing
            )
        {
            let checkpoint = current
                .progress
                .map(|progress| format!("{}:{}", progress.cursor.sequence, progress.cursor.digest))
                .unwrap_or_else(|| "bound".into());
            return Ok(Look::Alive {
                progress: format!("flux:{:?}:{checkpoint}", live.phase),
            });
        }
        // A historical event or local observer cannot turn an unknown native read into health.
        Ok(Look::Pending)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        match delivery.output {
            Output::Effect {
                kind: EffectKind::SubmitChecks,
                ..
            } => {
                let Some(job) = self
                    .shared
                    .transaction(|| self.shared.load(&correlation(delivery)))?
                else {
                    return Ok(Reconciliation::Absent);
                };
                if !job.matches(delivery)? {
                    return Err(Error::Invalid("check reconciliation identity changed"));
                }
                if let Some(completion) = job.completion {
                    return Ok(Reconciliation::Found(Box::new(completion)));
                }
                self.handle.watch(job.owner())?;
                Ok(Reconciliation::Pending)
            }
            Output::Effect {
                kind: EffectKind::WithdrawJob,
                ..
            } => {
                for job in self.targets(delivery)? {
                    if job.terminal.is_none() {
                        self.shared
                            .transaction(|| self.shared.fence(delivery.epoch))?;
                        self.shared
                            .backend
                            .cancel(&job.owner())
                            .map_err(adapter_error)?;
                        self.handle.watch(job.owner())?;
                        return Ok(Reconciliation::Pending);
                    }
                }
                Ok(Reconciliation::Found(Box::new(Completion::confirmed(
                    vec![],
                ))))
            }
            _ => Err(Error::Invalid("unsupported shared check reconciliation")),
        }
    }
}

pub mod activation;
pub mod staging;
