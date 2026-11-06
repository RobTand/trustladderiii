//! Event-driven controller observer ownership. Native execution stays with RunBackend.
use crate::{Result, cli::Input, run_backend::{Event, Observation, Owner, RunBackend, WatchStop}};
use crate::sync::mpsc::Sender as InputSender;
use std::{collections::BTreeMap, sync::{Arc, mpsc}, thread};

/// The adapter must journal its final completion before returning an ingress input.
/// This callback performs bounded local journal work, with no upstream wait or retry loop.
pub type Finish = Arc<dyn Fn(&Owner, Observation) -> Result<Option<Input>> + Send + Sync>;
/// Journal typed historical progress before publishing its ingress notification.
pub type Progress = Arc<dyn Fn(&Owner, &Event) -> Result<Option<Input>> + Send + Sync>;
pub type Backend = Arc<dyn RunBackend + Send + Sync>;

enum Command {
    Watch(Owner),
    Finished(String, Result<()>),
    Stop(Option<mpsc::SyncSender<()>>),
}

#[derive(Clone)]
pub struct Handle(mpsc::Sender<Command>);
impl Handle {
    /// Called only after the controller's immutable intent is durable.
    pub fn watch(&self, owner: Owner) -> Result<()> {
        self.0.send(Command::Watch(owner))
            .map_err(|_| crate::Error::Invalid("backend observer source closed"))
    }
}

pub struct Source {
    backend: Backend,
    commands: mpsc::Receiver<Command>,
    handle: Handle,
    recovered: Vec<Owner>,
    finish: Finish,
    progress: Progress,
}

struct Watch {
    owner: Owner,
    stop: WatchStop,
    thread: Option<thread::JoinHandle<Result<()>>>,
}
impl Watch {
    fn join(&mut self) -> Result<()> {
        self.thread.take().expect("owned observer thread").join()
            .map_err(|_| crate::Error::Invalid("backend observer panicked"))?
    }
}

pub struct RunningSource {
    handle: Handle,
    thread: Option<thread::JoinHandle<Result<()>>>,
}
impl RunningSource {
    /// Stop all local observers. Joining remains explicit or happens on drop.
    pub fn stop(&self) -> Result<()> {
        let (sender, stopped) = mpsc::sync_channel(0);
        self.handle.0.send(Command::Stop(Some(sender)))
            .map_err(|_| crate::Error::Invalid("backend source closed"))?;
        stopped.recv().map_err(|_| crate::Error::Invalid("backend source stopped before teardown"))
    }

    pub fn join(mut self) -> Result<()> {
        self.thread.take().expect("backend source thread").join()
            .map_err(|_| crate::Error::Invalid("backend source panicked"))?
    }
}
impl Drop for RunningSource {
    fn drop(&mut self) {
        let _ = self.handle.0.send(Command::Stop(None));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Source {
    /// Recovery supplies only durable owners; starting this source never submits work.
    pub fn new(backend: Backend, recovered: Vec<Owner>, finish: Finish) -> (Handle, Self) {
        let (sender, commands) = mpsc::channel();
        let handle = Handle(sender);
        (handle.clone(), Self { backend, commands, handle, recovered, finish, progress: Arc::new(|_, _| Ok(None)) })
    }

    pub fn with_progress(mut self, progress: Progress) -> Self {
        self.progress = progress;
        self
    }

    pub fn spawn(self, sender: InputSender<Input>) -> RunningSource {
        let handle = self.handle.clone();
        let thread = thread::spawn(move || self.run_inner(sender));
        RunningSource { handle, thread: Some(thread) }
    }

    fn run_inner(mut self, sender: InputSender<Input>) -> Result<()> {
        let mut watches = BTreeMap::<String, Watch>::new();
        let mut stopped = None;
        // Enqueue before waiting. Restart recovery follows the same attach boundary.
        for owner in self.recovered.drain(..) {
            self.handle.watch(owner)?;
        }
        let result = (|| -> Result<()> {
            while let Ok(command) = self.commands.recv() {
                match command {
                    Command::Stop(acknowledge) => {
                        stopped = acknowledge;
                        return Ok(());
                    }
                    Command::Finished(key, result) => {
                        if let Some(mut watch) = watches.remove(&key) {
                            watch.join()?;
                        }
                        result?;
                    }
                    Command::Watch(owner) => {
                        if let Some(watch) = watches.get(&owner.correlation) {
                            if watch.owner != owner {
                                return Err(crate::Error::Invalid("observer correlation belongs to another owner"));
                            }
                            continue;
                        }
                        let backend = self.backend.clone();
                        let finish = self.finish.clone();
                        let progress = self.progress.clone();
                        let sender = sender.clone();
                        let done = self.handle.0.clone();
                        let stop = WatchStop::default();
                        let watching = stop.clone();
                        let key = owner.correlation.clone();
                        let retained = owner.clone();
                        let thread = thread::spawn(move || {
                            // Always notify the source, including a callback panic, so it can reap us.
                            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
                                let observation = backend.attach(&owner, &watching, &mut |event| {
                                    if watching.is_stopped() { return Ok(()); }
                                    if let Some(input) = progress(&owner, event).map_err(|_| crate::run_backend::BackendError::unknown("controller progress journal unavailable"))? {
                                        sender.send(input).map_err(|_| crate::run_backend::BackendError::unknown("controller ingress closed"))?;
                                    }
                                    sender.send(Input::Wake).map_err(|_| crate::run_backend::BackendError::unknown("controller ingress closed"))
                                }).unwrap_or_else(|error| Observation::Unknown { binding: None,
                                    reason: format!("backend attach unavailable: {:?}", error.code) });
                                if watching.is_stopped() { return Ok(()); }
                                if let Some(input) = finish(&owner, observation)? {
                                    sender.send(input).map_err(|_| crate::Error::Invalid("controller ingress closed"))?;
                                }
                                Ok(())
                            })).unwrap_or(Err(crate::Error::Invalid("backend observer panicked")));
                            let _ = done.send(Command::Finished(key, result));
                            Ok(())
                        });
                        watches.insert(retained.correlation.clone(), Watch { owner: retained, stop, thread: Some(thread) });
                    }
                }
            }
            Ok(())
        })();
        // Stop every CLI before joining any one of them. Never issue a native cancel.
        for watch in watches.values() { watch.stop.stop(); }
        if let Some(stopped) = stopped { let _ = stopped.send(()); }
        let mut cleanup = Ok(());
        for watch in watches.values_mut() {
            if let Err(error) = watch.join() { cleanup = Err(error); }
        }
        result.and(cleanup)
    }
}

impl crate::ingress::Source for Source {
    fn run(self, sender: InputSender<Input>) -> Result<()> { self.run_inner(sender) }
}
