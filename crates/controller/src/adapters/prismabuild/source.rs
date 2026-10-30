use super::{Job, Settings, jobs, results};
use crate::{Result, cli::Input};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    thread,
    time::Duration,
};
use tl3_prismabuild::{ActionKey, ActionState, PrismaBuild, Watcher};

pub struct Source {
    client: PrismaBuild,
    settings: Settings,
    sent: BTreeSet<ActionKey>,
    watcher: Watcher,
    retry: BTreeSet<ActionKey>,
}

pub struct RunningSource {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Drop for RunningSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Source {
    pub(super) fn new(client: PrismaBuild, settings: Settings) -> Self {
        Self {
            watcher: client.watcher(),
            client,
            settings,
            sent: BTreeSet::new(),
            retry: BTreeSet::new(),
        }
    }

    /// Start one source thread. Only its receiver's owner applies observations to the controller.
    pub fn spawn(self, sender: Sender<Input>) -> RunningSource {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("source runtime");
            let mut source = self;
            while !stopping.load(Ordering::Relaxed) {
                if let Err(error) = runtime.block_on(source.poll(&sender, Duration::from_secs(1))) {
                    eprintln!("PrismaBuild ingress: {error}");
                    thread::park_timeout(Duration::from_millis(250));
                }
            }
        });
        RunningSource {
            stop,
            thread: Some(thread),
        }
    }

    pub async fn poll(&mut self, sender: &Sender<Input>, patience: Duration) -> Result<usize> {
        let own: BTreeMap<ActionKey, Job> = jobs(&self.settings.records)?
            .into_iter()
            .filter(|job| !self.sent.contains(&job.action))
            .map(|job| (job.action.clone(), job))
            .collect();
        let awaited = own.keys().cloned().collect();
        if own.is_empty() {
            tokio::time::sleep(patience).await;
            return Ok(0);
        }
        let changes = self
            .watcher
            .next(&awaited, patience)
            .await
            .map_err(super::adapter_error)?;
        let mut records = changes.records;
        for key in std::mem::take(&mut self.retry) {
            if own.contains_key(&key) && !records.contains_key(&key) {
                records.insert(key.clone(), self.client.read(&key).await);
            }
        }
        let mut count = 0;
        for (key, record) in records {
            let job = &own[&key];
            match record {
                Ok(Some(record))
                    if !matches!(record.state, ActionState::Queued | ActionState::Claimed) =>
                {
                    match results::ending(&self.settings.result_tool, job, false).await {
                        Ok(Some(completion)) => {
                            for observation in completion.observations {
                                sender
                                    .send(Input::Observation {
                                        key: format!("pb:{key}"),
                                        observation,
                                    })
                                    .map_err(|_| {
                                        crate::Error::Invalid("PrismaBuild ingress receiver closed")
                                    })?;
                            }
                            self.sent.insert(key);
                            count += 1;
                        }
                        Ok(None) => {
                            self.retry.insert(key);
                        }
                        Err(error) => {
                            eprintln!("PrismaBuild result {key}: {error}");
                            self.retry.insert(key);
                        }
                    }
                }
                Err(error) => {
                    eprintln!("PrismaBuild record {key}: {error}");
                    self.retry.insert(key);
                }
                _ => {}
            }
        }
        Ok(count)
    }
}
