use super::record::Records;
use crate::executor::Output as Out;
use crate::{
    Result,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation},
};
use std::path::Path;
use tl3_kernel::{EffectKind, NodeId};
use tl3_kernel::{Event, Reply};
use tl3_worktrees::{CommitId, Worktrees};

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
    sync::{Arc, Mutex},
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
    keys: tokio::sync::broadcast::Sender<NodeId>,
    records_lock: Arc<Mutex<()>>,
}

fn adapter_error(error: tl3_prismabuild::Error) -> crate::Error {
    crate::Error::Adapter(Box::new(error))
}

fn read_wait(
    client: &PrismaBuild,
    runtime: &tokio::runtime::Runtime,
    key: &str,
    checkout: &Path,
) -> Result<crate::executor::WaitRecord> {
    use crate::executor::WaitRecord;
    let Ok(key) = key.parse() else {
        return Ok(WaitRecord::Ended);
    };
    let Some(record) = runtime.block_on(client.read(&key)).map_err(adapter_error)? else {
        return Ok(WaitRecord::Ended);
    };
    if !matches!(record.state, ActionState::Queued | ActionState::Claimed) {
        return Ok(WaitRecord::Ended);
    }
    // PB v2 snapshots retain the submitted HEAD even when the original checkout is gone.
    let Some(parent) = record.snapshot_parent.and_then(|s| CommitId::new(s).ok()) else {
        return Ok(WaitRecord::Ended);
    };
    if !checkout.try_exists()? {
        return Ok(WaitRecord::Ended);
    }
    let alive = match Worktrees::open(checkout, checkout)
        .and_then(|repository| repository.is_ancestor(&parent, &repository.head()?))
    {
        Ok(alive) => alive,
        Err(tl3_worktrees::Error::Io) if !checkout.try_exists()? => false,
        Err(
            error @ (tl3_worktrees::Error::Spawn
            | tl3_worktrees::Error::Io
            | tl3_worktrees::Error::Poisoned),
        ) => return Err(error.into()),
        Err(_) => false,
    };
    Ok(if alive {
        WaitRecord::Alive
    } else {
        WaitRecord::Ended
    })
}

#[derive(Clone, Serialize, Deserialize)]
struct WaitSubscription {
    delivery: Delivery,
    wait: tl3_store::ExternalWait,
}

fn wait_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = vec![];
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(paths),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "wait") {
            paths.push(path);
        }
    }
    Ok(paths)
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
fn node_root(root: &Path, node: NodeId) -> PathBuf {
    root.join(node.0.to_string())
}
fn record_path(root: &Path, delivery: &Delivery, extension: &str) -> PathBuf {
    node_root(root, delivery.node()).join(format!("{}.{extension}", encoded(&delivery.key)))
}
fn node_job_paths(root: &Path, completed: bool) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
    };
    let mut found = vec![];
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|v| v == "job")
            && path.with_extension("done").exists() == completed
        {
            found.push(path);
        }
    }
    Ok(found)
}
fn job_paths(root: &Path, completed: bool) -> Result<Vec<PathBuf>> {
    let mut found = vec![];
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            found.extend(node_job_paths(&entry.path(), completed)?);
        }
    }
    Ok(found)
}

fn retire_superseded(root: &Path, delivery: &Delivery) -> Result<()> {
    let root = node_root(root, delivery.node());
    for path in node_job_paths(&root, false)?
        .into_iter()
        .chain(node_job_paths(&root, true)?)
    {
        let Some(job) = read_job(&path) else { continue };
        if job.delivery.generation < delivery.generation {
            // A later delivered check proves the prior effect and watch have settled.
            // Delete the binding first so a crash cannot expose a job without its done marker.
            fs::remove_file(&path)?;
            File::open(&root)?.sync_all()?;
            match fs::remove_file(path.with_extension("done")) {
                Ok(()) => File::open(&root)?.sync_all()?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

fn clean_inputs(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "input") {
            fs::remove_dir_all(path)?;
            File::open(root)?.sync_all()?;
        } else if path.extension().is_some_and(|ext| ext == "done")
            && !path.with_extension("job").exists()
        {
            fs::remove_file(path)?;
            File::open(root)?.sync_all()?;
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
        // Upgrade flat bindings once, preserving completions before moving their jobs.
        for entry in fs::read_dir(&settings.records)? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "job") {
                let job: Job = serde_json::from_slice(&fs::read(&path)?)?;
                let root = node_root(&settings.records, job.delivery.node());
                fs::create_dir_all(&root)?;
                File::open(&settings.records)?.sync_all()?;
                let done = path.with_extension("done");
                if done.try_exists()? {
                    fs::rename(&done, record_path(&settings.records, &job.delivery, "done"))?;
                    File::open(&root)?.sync_all()?;
                }
                fs::rename(&path, record_path(&settings.records, &job.delivery, "job"))?;
                File::open(&root)?.sync_all()?;
                File::open(&settings.records)?.sync_all()?;
            }
        }
        // Staging processes are parent-bound and cannot outlive the exclusive store claim.
        clean_inputs(&settings.records)?;
        for entry in fs::read_dir(&settings.records)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                clean_inputs(&entry.path())?;
            }
        }
        Ok(Self {
            client,
            settings,
            records_lock: Arc::new(Mutex::new(())),
            keys: tokio::sync::broadcast::channel(64).0,
            // Tool spawning stays on the daemon's controller owner thread. That thread
            // lives until daemon exit, as required by Linux's thread-scoped PDEATHSIG.
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
            self.records_lock.clone(),
        )
    }

    pub fn action_key(&self, delivery: &Delivery) -> Result<Option<ActionKey>> {
        Ok(self.job(delivery)?.map(|job| job.action))
    }

    fn job(&self, delivery: &Delivery) -> Result<Option<Job>> {
        match fs::read(record_path(&self.settings.records, delivery, "job")) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Call only after the request has closed and all its effects and watches have settled.
    /// Ticket #13 teardown owns this boundary. Repeated calls are harmless.
    pub fn retire_node(&mut self, node: NodeId) -> Result<()> {
        let _guard = self.records_lock.lock().expect("PrismaBuild records lock");
        let root = node_root(&self.settings.records, node);
        for path in node_job_paths(&root, false)? {
            // Corrupt bindings and failed upstream reads cannot establish termination.
            let job: Job = serde_json::from_slice(&fs::read(path)?)?;
            let record = self
                .runtime
                .block_on(self.client.read(&job.action))
                .map_err(adapter_error)?;
            if record.is_some_and(|record| {
                matches!(record.state, ActionState::Queued | ActionState::Claimed)
            }) {
                return Err(crate::Error::Invalid(
                    "cannot retire an active PrismaBuild job",
                ));
            }
        }
        // Remove bindings durably before their completions, including after partial retirement.
        for path in node_job_paths(&root, false)?
            .into_iter()
            .chain(node_job_paths(&root, true)?)
        {
            fs::remove_file(path)?;
        }
        match File::open(&root) {
            Ok(directory) => {
                directory.sync_all()?;
                fs::remove_dir_all(&root)?;
                File::open(&self.settings.records)?.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let _ = self.keys.send(node);
        Ok(())
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
        if let Some(job) = self.job(delivery)? {
            self.remove_input(delivery)?;
            return Ok(job);
        }
        let root = node_root(&self.settings.records, delivery.node());
        fs::create_dir_all(&root)?;
        File::open(&self.settings.records)?.sync_all()?;
        let input = record_path(&self.settings.records, delivery, "input");
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
        write(&record_path(&self.settings.records, delivery, "job"), &job)?;
        self.remove_input(delivery)?;
        Ok(job)
    }

    fn remove_input(&self, delivery: &Delivery) -> Result<()> {
        let input = record_path(&self.settings.records, delivery, "input");
        match fs::remove_dir_all(input) {
            Ok(()) => File::open(node_root(&self.settings.records, delivery.node()))?.sync_all()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    fn targets(&self, delivery: &Delivery) -> Result<Vec<Job>> {
        let mut targets = vec![];
        for path in node_job_paths(&node_root(&self.settings.records, delivery.node()), false)? {
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
        let done = record_path(&self.settings.records, &job.delivery, "done");
        match fs::read(done) {
            Ok(bytes) => return Ok(Reconciliation::Found(serde_json::from_slice(&bytes)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let record = self
            .runtime
            .block_on(self.client.read(&job.action))
            .map_err(adapter_error)?;
        if record.as_ref().is_some_and(|record| {
            matches!(record.state, ActionState::Queued | ActionState::Claimed)
        }) {
            return Ok(Reconciliation::Pending);
        }
        Ok(
            match self
                .runtime
                .block_on(results::ending(&self.settings.result_tool, job, false))?
            {
                Some(completion) => Reconciliation::Found(completion),
                None if record.is_none() => Reconciliation::Absent,
                None => Reconciliation::Pending,
            },
        )
    }
}

impl Executor for Adapter {
    fn watch_wait(
        &mut self,
        delivery: &Delivery,
        wait: Option<&tl3_store::ExternalWait>,
    ) -> Result<()> {
        let _guard = self.records_lock.lock().expect("PrismaBuild records lock");
        let path = record_path(&self.settings.records, delivery, "wait");
        let changed = if let Some(wait) = wait {
            if wait.upstream != "prismabuild" {
                return Ok(());
            }
            let _: ActionKey = wait.key.parse().map_err(adapter_error)?;
            let subscription = WaitSubscription {
                delivery: delivery.clone(),
                wait: wait.clone(),
            };
            let bytes = serde_json::to_vec(&subscription)?;
            if fs::read(&path).ok().as_ref() == Some(&bytes) {
                false
            } else {
                fs::create_dir_all(path.parent().unwrap())?;
                write(&path, &subscription)?;
                File::open(&self.settings.records)?.sync_all()?;
                true
            }
        } else {
            match fs::remove_file(&path) {
                Ok(()) => {
                    File::open(path.parent().unwrap())?.sync_all()?;
                    true
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error.into()),
            }
        };
        if changed {
            let _ = self.keys.send(delivery.node());
        }
        Ok(())
    }
    fn wait_record(&mut self, key: &str, checkout: &Path) -> Result<crate::executor::WaitRecord> {
        read_wait(&self.client, &self.runtime, key, checkout)
    }
    fn retire(&mut self, store: &mut tl3_store::Store) -> Result<()> {
        let snapshot = store.load()?;
        let world = crate::snapshot::decode(&snapshot)?;
        let pending = snapshot
            .pending_effects
            .iter()
            .filter(|effect| effect.kind == crate::executor::Upstream::PrismaBuild.kind())
            .map(|effect| serde_json::from_value::<Delivery>(effect.payload.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let watches = snapshot
            .rows
            .iter()
            .filter(|row| row.table == "watches")
            .map(|row| serde_json::from_value::<Delivery>(row.value.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut first_error = None;
        // Only retained adapter directories are candidates. Closed history costs no scan.
        for entry in fs::read_dir(&self.settings.records)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    first_error.get_or_insert_with(|| error.into());
                    continue;
                }
            };
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => {}
                Ok(_) => continue,
                Err(error) => {
                    first_error.get_or_insert_with(|| error.into());
                    continue;
                }
            }
            let Some(node) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u64>().ok())
                .map(NodeId)
            else {
                continue;
            };
            if !world
                .nodes
                .get(node.0 as usize)
                .is_some_and(|n| n.phase.is_closed())
            {
                continue;
            }
            if pending.iter().any(|d| d.node() == node)
                || watches.iter().any(|d| {
                    d.node() == node
                        && crate::executor::upstream(d.output)
                            == crate::executor::Upstream::PrismaBuild
                })
            {
                continue;
            }
            if let Err(error) = self.retire_node(node) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.fence(delivery.epoch)?;
        match delivery.output {
            Out::Effect {
                kind: EffectKind::SubmitChecks,
                ..
            } => {
                let job = self.submit(delivery)?;
                let _ = self.keys.send(delivery.node());
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
                self.remove_input(delivery)?;
                let targets = self.targets(delivery)?;
                for job in &targets {
                    self.remove_input(&job.delivery)?;
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
                self.remove_input(delivery)?;
                let Some(job) = self.job(delivery)? else {
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
                    self.remove_input(&job.delivery)?;
                    let Some(record) = self
                        .runtime
                        .block_on(self.client.read(&job.action))
                        .map_err(adapter_error)?
                    else {
                        // Share marker ownership with an ending already in flight on the source.
                        let _guard = self.records_lock.lock().expect("PrismaBuild records lock");
                        let done = record_path(&self.settings.records, &job.delivery, "done");
                        if !done.try_exists()? {
                            write(&done, &Completion::confirmed(vec![]))?;
                        }
                        let _ = self.keys.send(job.delivery.node());
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

#[cfg(test)]
mod git_tests {
    use super::*;
    use crate::git_tests::{Repo, wait_client};
    #[test]
    fn missing_checkout_ends_wait() {
        let f = Repo::new();
        let client = wait_client(f.root.path(), &f.base);
        let runtime = crate::executor::runtime().unwrap();
        fs::remove_dir_all(&f.path).unwrap();
        assert!(matches!(
            read_wait(&client, &runtime, &"a".repeat(64), &f.path).unwrap(),
            crate::executor::WaitRecord::Ended
        ));
    }
    #[test]
    fn hostile_read_wait() {
        crate::git_tests::hostile(
            "adapters::prismabuild::git_tests::hostile_read_wait",
            || {
                let f = Repo::new();
                let client = wait_client(f.root.path(), &f.head);
                let runtime = crate::executor::runtime().unwrap();
                assert!(matches!(
                    read_wait(&client, &runtime, &"a".repeat(64), &f.path).unwrap(),
                    crate::executor::WaitRecord::Alive
                ));
            },
        );
    }
}
