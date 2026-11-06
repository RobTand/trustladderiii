use std::{sync::{Arc, Condvar, Mutex, mpsc}, time::Duration};
use tl3_controller::{adapters::backend_watch::{Source, Finish}, cli::Input, run_backend::*};

struct Backend {
    entered: mpsc::Sender<(Owner, WatchStop)>,
    released: Arc<(Mutex<bool>, Condvar)>,
    fail: bool,
}
impl RunBackend for Backend {
    fn attach(&self, owner: &Owner, stop: &WatchStop, _: &mut dyn FnMut(&Event) -> Result<()>) -> Result<Observation> {
        self.entered.send((owner.clone(), stop.clone())).unwrap();
        if self.fail { return Err(BackendError::unknown("generation changed; retain owner")); }
        let (released, changed) = &*self.released;
        let mut released = released.lock().unwrap();
        while !*released { released = changed.wait(released).unwrap(); }
        Ok(Observation::Unknown { binding: None, reason: "test transport unavailable".into() })
    }
    fn submit(&self, _: Request, _: SubmissionIntent) -> Result<Submission> { panic!("observer must never submit") }
    fn cancel(&self, _: &Owner) -> Result<CancelAck> { panic!("local observer teardown must never cancel native execution") }
    fn probe(&self, _: &Reservation, _: &Placement) -> Result<Probe> { panic!("observer must never census") }
    fn liveness(&self, _: &Owner) -> Result<Liveness> { panic!("observer must not probe liveness") }
    fn validate_allocation(&self, _: &Owner, _: &AllocationInput) -> Result<CpuGrant> { panic!("observer must not qualify runtime") }
    fn worker_payload(&self, _: &Owner, _: &Terminal) -> Result<WorkerPayload> { panic!("observer must not interpret worker payload") }
    fn launch_context(&self, _: &Owner, _: &str) -> Result<LaunchContext> { panic!("observer must never launch") }
    fn retain_runtime(&self, _: &Owner, _: RuntimeOwnership) -> Result<()> { panic!("observer must never launch") }
    fn cleanup_runtime(&self, _: &Owner) -> Result<CleanupResult> { panic!("observer teardown must never clean native execution") }
    fn daytime_payload(&self, _: &Owner, _: &Terminal) -> Result<tl3_controller::contracts::report::CheckReport> { panic!("observer must not judge payloads") }
}
fn release(released: &Arc<(Mutex<bool>, Condvar)>) {
    *released.0.lock().unwrap() = true;
    released.1.notify_all();
}
fn owner() -> Owner { Owner { owner: "controller".into(), correlation: "retained-effect".into() } }

/// FX/#850: restart attaches the durable owner without resubmission; unknown stays unknown.
#[test]
fn observer_restart_attaches_and_retains_unknown_without_completion() {
    let (entered, attached) = mpsc::channel();
    let released = Arc::new((Mutex::new(false), Condvar::new()));
    let backend = Arc::new(Backend { entered, released: released.clone(), fail: false });
    let (finished, observed) = mpsc::channel();
    let finish: Finish = Arc::new(move |owner, observation| {
        assert!(matches!(observation, Observation::Unknown { .. }));
        finished.send(owner.clone()).unwrap();
        Ok(None)
    });
    let (_handle, source) = Source::new(backend, vec![owner()], finish);
    let (sender, inputs) = mpsc::channel();
    let running = source.spawn(sender);
    let entered = attached.recv_timeout(Duration::from_secs(5));
    // Release before any assertion so a failed test can still join the observer.
    release(&released);
    assert_eq!(entered.unwrap().0, owner());
    assert_eq!(observed.recv_timeout(Duration::from_secs(5)).unwrap(), owner());
    assert!(inputs.try_recv().is_err(), "unknown became a controller completion");
    drop(running);
}

/// FX: quiet observer shutdown stops every token and joins without native cancellation.
#[test]
fn observer_shutdown_stops_all_local_watches_before_joining() {
    let (entered, attached) = mpsc::channel();
    let released = Arc::new((Mutex::new(false), Condvar::new()));
    let backend = Arc::new(Backend { entered, released: released.clone(), fail: false });
    let finish: Finish = Arc::new(|_, _| panic!("stopped observation must not publish"));
    let mut second = owner();
    second.correlation = "second-effect".into();
    let (handle, source) = Source::new(backend, vec![owner(), second], finish);
    let (sender, _inputs) = mpsc::channel::<Input>();
    let running = source.spawn(sender);
    let first = attached.recv_timeout(Duration::from_secs(5));
    let second = attached.recv_timeout(Duration::from_secs(5));
    if first.is_err() || second.is_err() { release(&released); }
    let first = first.unwrap().1;
    let second = second.unwrap().1;
    // The stop acknowledgement is also a barrier behind these duplicate notifications.
    handle.watch(owner()).unwrap();
    handle.watch(owner()).unwrap();
    let stopped = running.stop();
    let both_stopped = first.is_stopped() && second.is_stopped();
    let duplicate = attached.try_recv().is_ok();
    release(&released);
    assert!(stopped.is_ok());
    assert!(both_stopped);
    assert!(!duplicate, "one owner acquired multiple concurrent observers");
    running.join().unwrap();
}


/// FX: an unavailable attach is retained as unknown without terminating unrelated observers.
#[test]
fn observer_attach_error_retains_unknown_and_keeps_source_available() {
    let (entered, _attached) = mpsc::channel();
    let backend = Arc::new(Backend { entered, released: Arc::new((Mutex::new(false), Condvar::new())), fail: true });
    let (finished, observed) = mpsc::channel();
    let finish: Finish = Arc::new(move |owner, observation| {
        assert!(matches!(observation, Observation::Unknown { binding: None, .. }));
        finished.send(owner.clone()).unwrap();
        Ok(None)
    });
    let (_handle, source) = Source::new(backend, vec![owner()], finish);
    let (sender, inputs) = mpsc::channel();
    let running = source.spawn(sender);
    assert_eq!(observed.recv_timeout(Duration::from_secs(5)).unwrap(), owner());
    assert!(inputs.try_recv().is_err());
    running.stop().unwrap();
    running.join().unwrap();
}
