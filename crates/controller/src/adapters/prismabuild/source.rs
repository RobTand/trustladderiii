use super::{Job, Settings, encoded, jobs, read_job, results, write};
use crate::{Result, cli::Input};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::mpsc::Sender,
    thread,
    time::Duration,
};
use tl3_prismabuild::{ActionKey, ActionState, PrismaBuild, Watcher};
use tokio::sync::{broadcast, oneshot};

pub struct Source {
    settings: Settings,
    watcher: Watcher,
    keys: broadcast::Receiver<String>,
    active: Option<BTreeMap<ActionKey, Job>>,
    tool_errors: BTreeSet<ActionKey>,
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

impl Source {
    pub(super) fn new(
        client: PrismaBuild,
        settings: Settings,
        keys: broadcast::Receiver<String>,
    ) -> Self {
        Self {
            watcher: client.watcher(),
            settings,
            keys,
            active: None,
            tool_errors: BTreeSet::new(),
        }
    }

    /// Start one source thread. Only its receiver's owner applies observations to the controller.
    pub fn spawn(self, sender: Sender<Input>) -> RunningSource {
        let (stop, stopping) = oneshot::channel();
        let (ready, initialized) = std::sync::mpsc::sync_channel(0);
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let mut source = self;
            let rebuilt = source.rebuild();
            let _ = ready.send(());
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
        });
        let _ = initialized.recv();
        RunningSource {
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    fn rebuild(&mut self) -> Result<()> {
        let mut active = BTreeMap::new();
        for (_, job) in jobs(&self.settings.records, false)? {
            active.insert(job.action.clone(), job);
        }
        self.tool_errors.retain(|key| active.contains_key(key));
        self.active = Some(active);
        Ok(())
    }

    fn done(&self, job: &Job) -> std::path::PathBuf {
        self.settings
            .records
            .join(format!("{}.done", encoded(&job.delivery.key)))
    }

    fn register(&mut self, key: String) -> Result<()> {
        let path = self.settings.records.join(format!("{}.job", encoded(&key)));
        if path.with_extension("done").exists() {
            return Ok(());
        }
        let Some(job) = read_job(&path) else {
            return Ok(());
        };
        if !self.done(&job).exists() {
            self.active
                .as_mut()
                .expect("initialized active set")
                .insert(job.action.clone(), job);
        }
        Ok(())
    }

    pub async fn poll(&mut self, sender: &Sender<Input>, patience: Duration) -> Result<usize> {
        if self.active.is_none() {
            self.rebuild()?;
        }
        let awaited = self.active.as_ref().unwrap().keys().cloned().collect();
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
            changes = self.watcher.next(&awaited, patience), if !self.active.as_ref().unwrap().is_empty() => {
                changes.map_err(super::adapter_error)?
            }
        };
        let mut count = 0;
        for (key, record) in changes.records {
            let job = &self.active.as_ref().unwrap()[&key];
            match record {
                Ok(Some(record))
                    if !matches!(record.state, ActionState::Queued | ActionState::Claimed) =>
                {
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
                    self.active.as_mut().unwrap().remove(&key);
                    self.tool_errors.remove(&key);
                    count += 1;
                }
                Err(error) => eprintln!("PrismaBuild record {key}: {error}"),
                _ => {}
            }
        }
        Ok(count)
    }
}
