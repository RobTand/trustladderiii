use super::{
    Job, Settings, WaitSubscription, job_paths, node_job_paths, node_root, read_job, record_path,
    results, retire_superseded, wait_paths, write,
};
use crate::sync::{Arc, Mutex, mpsc::Sender};
use crate::{Result, cli::Input};
use std::{
    collections::{BTreeMap, BTreeSet},
    thread,
    time::Duration,
};
use tl3_kernel::NodeId;
use tl3_prismabuild::{ActionKey, PrismaBuild, Watcher};
use tokio::sync::{broadcast, oneshot};

pub struct Source {
    settings: Settings,
    watcher: Watcher,
    keys: broadcast::Receiver<NodeId>,
    records_lock: Arc<Mutex<()>>,
    active: Option<BTreeMap<ActionKey, Job>>,
    tool_errors: BTreeSet<ActionKey>,
    waits: BTreeMap<String, WaitSubscription>,
}

pub struct RunningSource {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<Result<()>>>,
}
impl RunningSource {
    /// Propagate source errors and panics to the daemon's source supervisor.
    pub fn join(mut self) -> Result<()> {
        self.thread
            .take()
            .expect("running source thread")
            .join()
            .map_err(|_| crate::Error::Invalid("PrismaBuild source panicked"))?
    }
}
impl Drop for RunningSource {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl crate::ingress::Source for Source {
    fn run(self, sender: Sender<Input>) -> Result<()> {
        let (_stop, stopping) = oneshot::channel();
        self.run_with_stop(sender, stopping, None)
    }
}

impl Source {
    pub(super) fn new(
        client: PrismaBuild,
        settings: Settings,
        keys: broadcast::Receiver<NodeId>,
        records_lock: Arc<Mutex<()>>,
    ) -> Self {
        Self {
            watcher: client.watcher(),
            settings,
            keys,
            records_lock,
            active: None,
            tool_errors: BTreeSet::new(),
            waits: BTreeMap::new(),
        }
    }

    /// Start one source thread. Only its receiver's owner applies observations to the controller.
    pub fn spawn(self, sender: Sender<Input>) -> RunningSource {
        let (stop, stopping) = oneshot::channel();
        let (ready, initialized) = std::sync::mpsc::sync_channel(0);
        let thread = thread::spawn(move || self.run_with_stop(sender, stopping, Some(ready)));
        let _ = initialized.recv();
        RunningSource {
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    fn run_with_stop(
        self,
        sender: Sender<Input>,
        stopping: oneshot::Receiver<()>,
        ready: Option<std::sync::mpsc::SyncSender<()>>,
    ) -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let mut source = self;
        let rebuilt = source.rebuild();
        if let Some(ready) = ready {
            let _ = ready.send(());
        }
        let result = rebuilt.and_then(|()| {
            runtime.block_on(async {
                tokio::pin!(stopping);
                loop {
                    tokio::select! {
                        _ = &mut stopping => return Ok(()),
                        result = source.poll(&sender, Duration::from_secs(300)) => {
                            result?;
                        }
                    }
                }
            })
        });
        if let Err(error) = &result {
            eprintln!("PrismaBuild ingress stopped: {error}");
        }
        result
    }

    fn rebuild(&mut self) -> Result<()> {
        let mut active = BTreeMap::new();
        for path in job_paths(&self.settings.records, false)? {
            if let Some(job) = read_job(&path) {
                active.insert(job.action.clone(), job);
            }
        }
        self.tool_errors.retain(|key| active.contains_key(key));
        self.active = Some(active);
        self.waits.clear();
        for entry in std::fs::read_dir(&self.settings.records)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                self.register_waits(entry.path())?;
            }
        }
        Ok(())
    }

    fn done(&self, job: &Job) -> std::path::PathBuf {
        record_path(&self.settings.records, &job.delivery, "done")
    }

    fn register_waits(&mut self, root: std::path::PathBuf) -> Result<()> {
        let _guard = self.records_lock.lock().expect("PrismaBuild records lock");
        for path in wait_paths(&root)? {
            let result = (|| -> Result<WaitSubscription> {
                let wait: WaitSubscription = serde_json::from_slice(&std::fs::read(&path)?)?;
                let _: ActionKey = wait.wait.key.parse().map_err(super::adapter_error)?;
                Ok(wait)
            })();
            match result {
                Ok(wait) => {
                    self.waits.insert(wait.delivery.key.clone(), wait);
                }
                Err(error) => eprintln!("PrismaBuild wait {} skipped: {error}", path.display()),
            }
        }
        Ok(())
    }

    fn register(&mut self, node: NodeId) -> Result<()> {
        let active = self.active.as_mut().expect("initialized active set");
        active.retain(|_, job| job.delivery.node() != node);
        for path in node_job_paths(&node_root(&self.settings.records, node), false)? {
            if let Some(job) = read_job(&path) {
                active.insert(job.action.clone(), job);
            }
        }
        self.tool_errors
            .retain(|action| active.contains_key(action));
        self.waits.retain(|_, wait| wait.delivery.node() != node);
        self.register_waits(node_root(&self.settings.records, node))?;
        Ok(())
    }

    pub async fn poll(&mut self, sender: &Sender<Input>, patience: Duration) -> Result<usize> {
        if self.active.is_none() {
            self.rebuild()?;
        }
        let awaited: BTreeSet<_> = self
            .active
            .as_ref()
            .unwrap()
            .keys()
            .cloned()
            .chain(self.waits.values().map(|wait| {
                wait.wait
                    .key
                    .parse::<ActionKey>()
                    .expect("validated wait key")
            }))
            .collect();
        let changes = tokio::select! {
            key = self.keys.recv() => {
                match key {
                    Ok(key) => self.register(key)?,
                    // The channel is bounded. Durable records recover notifications lost to lag.
                    Err(broadcast::error::RecvError::Lagged(_)) => self.rebuild()?,
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(crate::Error::Invalid("PrismaBuild adapter channel closed"));
                    }
                }
                return Ok(0);
            }
            changes = self.watcher.next(&awaited, patience), if !awaited.is_empty() => {
                changes.map_err(super::adapter_error)?
            }
        };
        let mut count = 0;
        for (key, record) in changes.records {
            let ended = record.as_ref().is_ok_and(|record| !record.is_live());
            if ended || record.is_err() {
                for wait in self
                    .waits
                    .values()
                    .filter(|wait| wait.wait.key == key.as_str())
                {
                    sender
                        .send(Input::Look {
                            key: wait.delivery.key.clone(),
                        })
                        .map_err(|_| {
                            crate::Error::Invalid("PrismaBuild ingress receiver closed")
                        })?;
                    count += 1;
                }
            }
            let Some(job) = self.active.as_ref().unwrap().get(&key) else {
                continue; // A newer completion in this batch retired the job.
            };
            match record {
                Ok(record) if !record.is_live() => {
                    // Bell transport backoff is not news about a failed result tool.
                    if self.tool_errors.contains(&key) && !changes.bell_available {
                        continue;
                    }
                    let completion =
                        match results::ending(&self.settings.result_tool, job, false).await {
                            Ok(Some(completion)) => completion,
                            Ok(None) => continue,
                            Err(error) => {
                                eprintln!("PrismaBuild result {key}: {error}");
                                self.tool_errors.insert(key.clone());
                                continue;
                            }
                        };
                    // Teardown may run as soon as the controller consumes this observation.
                    // Hold the lock through delivery and marker fsync, and discard cached
                    // jobs removed by teardown while the result tool was running.
                    let _guard = self.records_lock.lock().expect("PrismaBuild records lock");
                    if !record_path(&self.settings.records, &job.delivery, "job").try_exists()?
                        || self.done(job).is_file()
                    {
                        self.active.as_mut().unwrap().remove(&key);
                        self.tool_errors.remove(&key);
                        continue;
                    }
                    for observation in &completion.observations {
                        sender
                            .send(Input::Observation {
                                key: format!("pb:{key}"),
                                observation: observation.clone(),
                            })
                            .map_err(|_| {
                                crate::Error::Invalid("PrismaBuild ingress receiver closed")
                            })?;
                    }
                    // Retain the completion for controller watches if it crashes before consuming Input.
                    write(&self.done(job), &completion)?;
                    retire_superseded(&self.settings.records, &job.delivery)?;
                    let node = job.delivery.node();
                    let generation = job.delivery.generation;
                    self.active.as_mut().unwrap().remove(&key);
                    self.active.as_mut().unwrap().retain(|_, job| {
                        job.delivery.node() != node || job.delivery.generation >= generation
                    });
                    self.tool_errors
                        .retain(|key| self.active.as_ref().unwrap().contains_key(key));
                    count += 1;
                }
                Err(error) => eprintln!("PrismaBuild record {key}: {error}"),
                _ => {}
            }
        }
        Ok(count)
    }
}

#[cfg(all(test, loom))]
#[path = "../../../tests/loom/bell.rs"]
mod loom_tests;
