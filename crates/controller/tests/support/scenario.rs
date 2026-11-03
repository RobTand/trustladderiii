use std::{
    cell::RefCell,
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Progress {
    marker: String,
    last: Instant,
    max_inactivity: Duration,
    finished: Option<Instant>,
}

struct Watchdog {
    progress: Mutex<Progress>,
    changed: Condvar,
    inactivity: Duration,
    deadline: Instant,
}

type Active = Arc<Watchdog>;
thread_local! {
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
}

struct Guard {
    previous: Option<Active>,
    active: Active,
    watcher: Option<thread::JoinHandle<()>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        ACTIVE.with(|active| active.replace(self.previous.take()));
        self.active.progress.lock().unwrap().finished = Some(Instant::now());
        self.active.changed.notify_one();
        self.watcher.take().unwrap().join().unwrap();
    }
}

pub fn run<T>(name: &str, inactivity: Duration, scenario: impl FnOnce() -> T) -> T {
    run_with_limits(name, inactivity, Duration::from_secs(600), scenario)
}

// Test seam for exercising the absolute cap without a ten-minute fixture.
pub fn run_with_limits<T>(
    name: &str,
    inactivity: Duration,
    absolute: Duration,
    scenario: impl FnOnce() -> T,
) -> T {
    let started = Instant::now();
    let active = Arc::new(Watchdog {
        progress: Mutex::new(Progress {
            marker: "scenario entered".to_owned(),
            last: started,
            max_inactivity: Duration::ZERO,
            finished: None,
        }),
        changed: Condvar::new(),
        inactivity,
        deadline: started + absolute,
    });
    let previous = ACTIVE.with(|current| current.replace(Some(active.clone())));
    let name = name.to_owned();
    eprintln!(
        "scenario start: {name}; unix_ms={}; inactivity_limit={inactivity:?}; wall_limit={absolute:?}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let watched = active.clone();
    let watcher = thread::spawn(move || {
        let mut progress = watched.progress.lock().unwrap();
        loop {
            let now = progress.finished.unwrap_or_else(Instant::now);
            let gap = now.duration_since(progress.last);
            let max_inactivity = progress.max_inactivity.max(gap);
            if gap >= inactivity || now >= watched.deadline {
                eprintln!(
                    "scenario watchdog expired: {name}; elapsed={:?}; last progress: {}; inactivity={gap:?}; max_inactivity={max_inactivity:?}",
                    now.duration_since(started),
                    progress.marker
                );
                // A panic on this thread cannot stop a blocked scenario thread.
                // Bazel and PrismaBuild own cleanup of the test's process scope.
                std::process::exit(124);
            }
            if progress.finished.is_some() {
                eprintln!(
                    "scenario complete: {name}; elapsed={:?}; max_inactivity={max_inactivity:?}",
                    now.duration_since(started)
                );
                return;
            }
            let remaining = (inactivity - gap).min(watched.deadline.duration_since(now));
            progress = watched.changed.wait_timeout(progress, remaining).unwrap().0;
        }
    });
    let _guard = Guard {
        previous,
        active,
        watcher: Some(watcher),
    };
    scenario()
}

pub fn progress(marker: impl Into<String>) {
    ACTIVE.with(|active| {
        if let Some(active) = active.borrow().as_ref() {
            let mut progress = active.progress.lock().unwrap();
            let now = Instant::now();
            let gap = now.duration_since(progress.last);
            // Late progress cannot erase a stall if the watcher was descheduled.
            if gap < active.inactivity && now < active.deadline {
                progress.max_inactivity = progress.max_inactivity.max(gap);
                progress.last = now;
                progress.marker = marker.into();
            }
            active.changed.notify_one();
        }
    });
}

#[track_caller]
pub fn wait<T>(
    name: &str,
    inactivity: Duration,
    absolute: Duration,
    mut condition: impl FnMut() -> Option<T>,
) -> T {
    run_with_limits(name, inactivity, absolute, || {
        progress(format!("waiting for {name}"));
        loop {
            if let Some(value) = condition() {
                return value;
            }
            thread::sleep(Duration::from_millis(10));
        }
    })
}
