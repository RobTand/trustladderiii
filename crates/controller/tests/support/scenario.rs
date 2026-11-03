use std::{
    cell::RefCell,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Progress = Arc<Mutex<String>>;
thread_local! {
    static ACTIVE: RefCell<Option<Progress>> = const { RefCell::new(None) };
}

struct Guard {
    previous: Option<Progress>,
    done: mpsc::Sender<Instant>,
    watcher: Option<thread::JoinHandle<()>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        ACTIVE.with(|active| active.replace(self.previous.take()));
        let _ = self.done.send(Instant::now());
        self.watcher.take().unwrap().join().unwrap();
    }
}

pub fn run<T>(name: &str, limit: Duration, scenario: impl FnOnce() -> T) -> T {
    let limit = limit.min(Duration::from_secs(120));
    let started = Instant::now();
    let marker = Arc::new(Mutex::new("scenario entered".to_owned()));
    let previous = ACTIVE.with(|active| active.replace(Some(marker.clone())));
    let (done, receiver) = mpsc::channel::<Instant>();
    let name = name.to_owned();
    eprintln!(
        "scenario start: {name}; unix_ms={}; wall_limit={limit:?}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let watcher = thread::spawn(move || {
        match receiver.recv_timeout(limit.saturating_sub(started.elapsed())) {
            Ok(finished) if finished.duration_since(started) <= limit => return,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            _ => {}
        }
        eprintln!(
            "scenario watchdog expired: {name}; elapsed={:?}; last progress: {}",
            started.elapsed(),
            marker.lock().unwrap()
        );
        // A panic on this thread cannot stop a blocked scenario thread.
        // Bazel and PrismaBuild own cleanup of the test's process scope.
        std::process::exit(124);
    });
    let _guard = Guard {
        previous,
        done,
        watcher: Some(watcher),
    };
    scenario()
}

pub fn progress(marker: impl Into<String>) {
    ACTIVE.with(|active| {
        if let Some(active) = active.borrow().as_ref() {
            *active.lock().unwrap() = marker.into();
        }
    });
}
