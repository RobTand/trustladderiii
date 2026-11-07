//! Worker observation never enters the daytime completion handler.
use super::*;
use crate::adapters::{backend_watch, flux_checks::WorkerWaits};
use crate::executor::WaitRecord;
use std::sync::Arc;

struct Shared {
    root: PathBuf,
    options: backend::Options,
    backend: backend_watch::Backend,
}

impl Shared {
    fn path(&self, key: &str) -> Result<PathBuf> {
        let id = key
            .strip_prefix("worker:")
            .filter(|id| backend::hex(id, 64))
            .ok_or(Error::Invalid(
                "Flux waits require an opaque worker reference",
            ))?;
        Ok(self.root.join(format!("{id}.json")))
    }

    fn load(&self, key: &str) -> Result<WaitBinding> {
        Ok(serde_json::from_slice(
            &durable::read(&self.path(key)?).map_err(backend_error)?,
        )?)
    }

    fn owned(&self, owner: &backend::Owner) -> Result<(String, WaitBinding, Profile)> {
        let id = owner
            .correlation
            .strip_prefix("worker-")
            .filter(|id| backend::hex(id, 64))
            .ok_or(Error::Invalid(
                "worker observer has no retained worker reference",
            ))?;
        let key = format!("worker:{id}");
        let wait = self.load(&key)?;
        let profile: Profile =
            serde_json::from_slice(&durable::read(&wait.profile).map_err(backend_error)?)?;
        wait.validate(&key, &profile.checkout, &self.options)?;
        if wait.owner != *owner {
            return Err(Error::Invalid(
                "worker observation belongs to another owner",
            ));
        }
        Ok((key, wait, profile))
    }

    fn retain_progress(
        &self,
        owner: &backend::Owner,
        event: &backend::Event,
    ) -> Result<Option<crate::cli::Input>> {
        let (key, wait, _) = self.owned(owner)?;
        let record = durable::Journal::new(self.options.journal_root.clone())
            .map_err(backend_error)?
            .load(owner)
            .map_err(backend_error)?;
        if event.progress.owner != *owner
            || event.progress.cursor.sequence != event.sequence
            || event.progress.cursor.digest != event.digest
            || event.progress.authority != backend::ProgressAuthority::Historical
            || record.binding.as_ref().map(|binding| &binding.id) != Some(&event.progress.run)
            || record.intent != wait.intent
        {
            return Err(Error::Invalid(
                "worker progress differs from its retained native binding",
            ));
        }
        // Immutable event files make replay idempotent and keep concurrent writes apart.
        let path = self
            .path(&key)?
            .with_extension(format!("progress-{}.json", event.sequence));
        let bytes = serde_json::to_vec(&event.progress)?;
        match fs::read(&path) {
            Ok(existing) if existing == bytes => {}
            Ok(_) => return Err(Error::Invalid("worker progress cursor conflict")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                private_create(&path, &bytes)?
            }
            Err(error) => return Err(error.into()),
        }
        Ok(None)
    }

    fn retain_observation(
        &self,
        owner: &backend::Owner,
        observed: backend::Observation,
    ) -> Result<Option<crate::cli::Input>> {
        let (key, wait, _) = self.owned(owner)?;
        let binding = match &observed {
            backend::Observation::Terminal { binding, .. }
            | backend::Observation::Unknown { binding, .. } => binding,
        };
        if binding.as_ref().is_some_and(|binding| {
            binding.intent != wait.intent
                || wait
                    .binding
                    .as_ref()
                    .is_some_and(|retained| retained != binding)
        }) {
            return Err(Error::Invalid("worker observation binding changed"));
        }
        durable::atomic(
            &self.path(&key)?.with_extension("observation.json"),
            &serde_json::to_vec(&observed)?,
        )
        .map_err(backend_error)?;
        Ok(Some(crate::cli::Input::Wake))
    }
}

pub struct Waits {
    shared: Arc<Shared>,
    watcher: backend_watch::Handle,
    subscriptions: BTreeMap<String, Vec<backend::Owner>>,
}

impl Waits {
    /// Composition returns a separate owner-neutral source. Its Finish only wakes workers.
    pub fn new(
        backend: backend_watch::Backend,
        options: backend::Options,
        root: &Path,
    ) -> Result<(Self, backend_watch::Source)> {
        fs::create_dir_all(root)?;
        let shared = Arc::new(Shared {
            root: root.to_owned(),
            options,
            backend: backend.clone(),
        });
        let mut recovered = Vec::new();
        let mut subscriptions = BTreeMap::<String, Vec<backend::Owner>>::new();
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            let Some(id) = path
                .file_stem()
                .and_then(|id| id.to_str())
                .filter(|id| backend::hex(id, 64))
            else {
                continue;
            };
            let wait = shared.load(&format!("worker:{id}"))?;
            // A pre-dispatch crash retains private intent/input but establishes no native job.
            // Keep that held reference without starting an observer or blocking other workers.
            let submitted = durable::Journal::new(shared.options.journal_root.clone())
                .map_err(backend_error)?
                .transaction(&wait.owner, |_, record| Ok(record.is_some()))
                .map_err(backend_error)?;
            if !submitted {
                continue;
            }
            let (_, _, profile) = shared.owned(&wait.owner)?;
            subscriptions
                .entry(durable::digest(&serde_json::to_vec(&profile.delivery)?))
                .or_default()
                .push(wait.owner.clone());
            recovered.push(wait.owner);
        }
        let finishing = shared.clone();
        let progress = shared.clone();
        let (watcher, source) = backend_watch::Source::new(
            backend,
            recovered,
            Arc::new(move |owner, observation| finishing.retain_observation(owner, observation)),
        );
        Ok((
            Self {
                shared,
                watcher,
                subscriptions,
            },
            source.with_progress(Arc::new(move |owner, event| {
                progress.retain_progress(owner, event)
            })),
        ))
    }

    /// Persist before dispatch. Unknown acceptance retains the same reference and inputs.
    pub fn retain(&self, wait: &WaitBinding) -> Result<String> {
        let key = wait.key()?;
        let path = self.shared.path(&key)?;
        let bytes = serde_json::to_vec(wait)?;
        match fs::read(&path) {
            Ok(existing) if existing == bytes => Ok(key),
            Ok(_) => Err(Error::Invalid(
                "worker reference already has another binding",
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                private_create(&path, &bytes)?;
                Ok(key)
            }
            Err(error) => Err(error.into()),
        }
    }
}

impl WorkerWaits for Waits {
    fn watch_wait(
        &mut self,
        delivery: &Delivery,
        wait: Option<&tl3_store::ExternalWait>,
    ) -> Result<()> {
        let subscription = durable::digest(&serde_json::to_vec(delivery)?);
        let Some(wait) = wait else {
            if let Some(owners) = self.subscriptions.get(&subscription) {
                for owner in owners {
                    self.watcher.unwatch(owner.clone())?;
                }
                self.subscriptions.remove(&subscription);
            }
            return Ok(());
        };
        if wait.upstream != "flux" {
            return Err(Error::Invalid(
                "selected Flux cannot reinterpret a PB worker wait",
            ));
        }
        let retained = self.shared.load(&wait.key)?;
        let (_, _, profile) = self.shared.owned(&retained.owner)?;
        if serde_json::to_vec(&profile.delivery)? != serde_json::to_vec(delivery)? {
            return Err(Error::Invalid(
                "worker wait belongs to another delivery attempt",
            ));
        }
        if let Some(previous) = self.subscriptions.get(&subscription) {
            for owner in previous.iter().filter(|owner| **owner != retained.owner) {
                self.watcher.unwatch(owner.clone())?;
            }
        }
        self.watcher.watch(retained.owner.clone())?;
        self.subscriptions
            .insert(subscription, vec![retained.owner]);
        Ok(())
    }

    fn wait_record(&mut self, key: &str, checkout: &Path) -> Result<WaitRecord> {
        let wait = self.shared.load(key)?;
        wait.validate(key, checkout, &self.shared.options)?;
        let live = self
            .shared
            .backend
            .liveness(&wait.owner)
            .map_err(backend_error)?;
        let record = durable::Journal::new(self.shared.options.journal_root.clone())
            .map_err(backend_error)?
            .load(&wait.owner)
            .map_err(backend_error)?;
        if live.owner != wait.owner
            || live.run.as_ref() != record.binding.as_ref().map(|binding| &binding.id)
        {
            return Err(Error::Invalid(
                "worker liveness belongs to another retained native run",
            ));
        }
        if live.authority == backend::ProgressAuthority::CommittedTerminal
            && live.phase == backend::NativePhase::Terminal
            && live.terminal.is_some()
            && live.terminal == record.terminal
        {
            return Ok(WaitRecord::Ended);
        }
        if live.authority == backend::ProgressAuthority::FreshNative
            && live.run.is_some()
            && live.evidence.is_some()
            && live.observed_unix_ms > 0
            && matches!(
                live.phase,
                backend::NativePhase::Queued
                    | backend::NativePhase::Running
                    | backend::NativePhase::Finishing
            )
        {
            return Ok(WaitRecord::Alive);
        }
        Err(Error::Invalid(
            "worker native liveness is uncertain; retain binding and inputs",
        ))
    }
}
