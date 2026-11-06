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
    Unwatch(Owner, mpsc::SyncSender<Result<()>>),
    Finished(String, u64, Result<()>),
    Stop(Option<mpsc::SyncSender<()>>),
}

#[derive(Clone)]
pub struct Handle(mpsc::Sender<Command>);
impl Handle {
    /// Detach and join one local observer. Native execution remains owned and reattachable.
    pub fn unwatch(&self, owner: Owner) -> Result<()> {
        let (acknowledge, detached) = mpsc::sync_channel(1);
        self.0.send(Command::Unwatch(owner, acknowledge))
            .map_err(|_| crate::Error::Invalid("backend observer source closed"))?;
        detached.recv().map_err(|_| crate::Error::Invalid("backend source closed before detach acknowledgement"))?
    }
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
    identity: u64,
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
        let mut next_identity = 0u64;
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
                    Command::Unwatch(owner, acknowledge) => {
                        let result = if watches.get(&owner.correlation).is_some_and(|watch| watch.owner != owner) {
                            Err(crate::Error::Invalid("observer correlation belongs to another owner"))
                        } else if let Some(mut watch) = watches.remove(&owner.correlation) {
                            watch.stop.stop();
                            watch.join()
                        } else { Ok(()) };
                        let _ = acknowledge.send(result);
                    }
                    Command::Finished(key, identity, result) => {
                        if watches.get(&key).is_some_and(|watch| watch.identity == identity) {
                            let mut watch = watches.remove(&key).expect("matched local observer identity");
                            watch.join()?;
                            result?;
                        }
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
                        let identity = next_identity;
                        next_identity = next_identity.checked_add(1).ok_or(crate::Error::Invalid("observer identity exhausted"))?;
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
                            let _ = done.send(Command::Finished(key, identity, result));
                            Ok(())
                        });
                        watches.insert(retained.correlation.clone(), Watch { identity, owner: retained, stop, thread: Some(thread) });
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

#[cfg(test)]
mod detach_tests {
    use super::*;
    use crate::run_backend::*;
    use std::{os::unix::process::CommandExt, process::{Command as Process, Stdio}, time::Duration};

    struct Attachment {
        owner: Owner,
        stop: WatchStop,
        exited: mpsc::Receiver<()>,
        release: mpsc::Sender<()>,
    }
    impl Drop for Attachment {
        fn drop(&mut self) { self.stop.stop(); let _ = self.release.send(()); }
    }
    struct Quiet(mpsc::Sender<Attachment>);
    impl RunBackend for Quiet {
        fn attach(&self, owner: &Owner, stop: &WatchStop, emit: &mut dyn FnMut(&Event) -> crate::run_backend::Result<()>) -> crate::run_backend::Result<Observation> {
            let mut child = Process::new("cat").stdin(Stdio::piped()).stdout(Stdio::null()).process_group(0).spawn().unwrap();
            let _input = child.stdin.take();
            stop.register(child.id());
            let (release, blocked) = mpsc::channel();
            let (exited, ended) = mpsc::channel();
            self.0.send(Attachment { owner: owner.clone(), stop: stop.clone(), exited: ended, release }).unwrap();
            child.wait().unwrap();
            stop.unregister(child.id());
            exited.send(()).unwrap();
            blocked.recv().unwrap();
            let run = BackendId { backend: BackendTag::Flux, uri: "local:///fixture".into(), instance: "fixture".into(), job: "fABC".into() };
            emit(&Event { sequence: 0, digest: "a".repeat(64), fact: NativeFact::Start,
                progress: NativeProgress { owner: owner.clone(), run, cursor: EventCursor { sequence: 0, digest: "a".repeat(64) },
                    phase: NativePhase::Running, authority: ProgressAuthority::Historical } })?;
            Ok(Observation::Unknown { binding: None, reason: "local observer stopped".into() })
        }
        fn submit(&self, _: Request, _: SubmissionIntent) -> crate::run_backend::Result<Submission> { panic!("no submit") }
        fn cancel(&self, _: &Owner) -> crate::run_backend::Result<CancelAck> { panic!("no native cancel") }
        fn probe(&self, _: &Reservation, _: &Placement) -> crate::run_backend::Result<Probe> { panic!("no probe") }
        fn liveness(&self, _: &Owner) -> crate::run_backend::Result<Liveness> { panic!("no native query") }
        fn launch_context(&self, _: &Owner, _: &str) -> crate::run_backend::Result<LaunchContext> { panic!("no launch") }
        fn retain_runtime(&self, _: &Owner, _: RuntimeOwnership) -> crate::run_backend::Result<()> { panic!("no runtime mutation") }
        fn validate_allocation(&self, _: &Owner, _: &AllocationInput) -> crate::run_backend::Result<CpuGrant> { panic!("no qualification") }
        fn cleanup_runtime(&self, _: &Owner) -> crate::run_backend::Result<CleanupResult> { panic!("no cleanup") }
        fn worker_payload(&self, _: &Owner, _: &Terminal) -> crate::run_backend::Result<WorkerPayload> { panic!("no payload") }
        fn daytime_payload(&self, _: &Owner, _: &Terminal) -> crate::run_backend::Result<crate::contracts::report::CheckReport> { panic!("no daytime result") }
    }

    fn owner(correlation: &str) -> Owner { Owner { owner: "worker-tools".into(), correlation: correlation.into() } }
    fn take(receiver: &mpsc::Receiver<Attachment>) -> Attachment { receiver.recv_timeout(Duration::from_secs(5)).unwrap() }

    #[test]
    fn quiet_two_owner_detach_joins_before_ack_and_preserves_other_owner() {
        let (entered, attached) = mpsc::channel();
        let (handle, source) = Source::new(Arc::new(Quiet(entered)), vec![owner("first"), owner("second")],
            Arc::new(|_, _| panic!("detached watcher published finish")));
        let source = source.with_progress(Arc::new(|_, _| panic!("detached watcher published progress")));
        let (sender, _inputs) = crate::sync::mpsc::channel();
        let running = source.spawn(sender);
        let first = take(&attached);
        let second = take(&attached);
        let (done, detached) = mpsc::channel();
        let detaching = handle.clone();
        let target = first.owner.clone();
        let thread = thread::spawn(move || done.send(detaching.unwatch(target)).unwrap());
        first.exited.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(first.stop.is_stopped());
        assert!(!second.stop.is_stopped());
        assert!(detached.try_recv().is_err(), "detach acknowledged before join");
        first.release.send(()).unwrap();
        detached.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        thread.join().unwrap();
        let mut wrong = second.owner.clone(); wrong.owner = "unrelated".into();
        assert!(handle.unwatch(wrong).is_err());
        assert!(!second.stop.is_stopped());
        handle.unwatch(first.owner.clone()).unwrap();
        second.stop.stop();
        second.exited.recv_timeout(Duration::from_secs(5)).unwrap();
        second.release.send(()).unwrap();
        running.stop().unwrap(); running.join().unwrap();
    }

    #[test]
    fn late_finished_cannot_remove_or_fail_a_replacement_watch() {
        let (entered, attached) = mpsc::channel();
        let (handle, source) = Source::new(Arc::new(Quiet(entered)), vec![owner("same")], Arc::new(|_, _| panic!("finish after detach")));
        let (sender, _inputs) = crate::sync::mpsc::channel();
        let running = source.spawn(sender);
        let old = take(&attached);
        let (ack, detached) = mpsc::sync_channel(1);
        handle.0.send(Command::Unwatch(old.owner.clone(), ack)).unwrap();
        old.exited.recv_timeout(Duration::from_secs(5)).unwrap();
        // Queue replacement while detach is blocked joining the old local watcher.
        handle.watch(old.owner.clone()).unwrap();
        old.release.send(()).unwrap();
        detached.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        let replacement = take(&attached);
        handle.0.send(Command::Finished(old.owner.correlation.clone(), 0,
            Err(crate::Error::Invalid("stale detached failure")))).unwrap();
        let (ack, barrier) = mpsc::sync_channel(1);
        handle.0.send(Command::Unwatch(owner("absent"), ack)).unwrap();
        let passed_stale = barrier.recv_timeout(Duration::from_secs(5));
        let replacement_alive = !replacement.stop.is_stopped();
        replacement.stop.stop();
        replacement.exited.recv_timeout(Duration::from_secs(5)).unwrap();
        replacement.release.send(()).unwrap();
        let stopped = running.stop();
        let joined = running.join();
        passed_stale.unwrap().unwrap();
        assert!(replacement_alive);
        stopped.unwrap(); joined.unwrap();
    }
}
