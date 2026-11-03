use std::time::{Duration, Instant};

pub struct DrainDeadline {
    start: Instant,
    last_progress: Instant,
    inactivity: Duration,
}

impl DrainDeadline {
    pub fn new(start: Instant, inactivity: Duration) -> Self {
        Self {
            start,
            last_progress: start,
            inactivity,
        }
    }

    pub fn progress(&mut self, now: Instant) {
        self.check(now);
        self.last_progress = now;
    }

    pub fn check(&self, now: Instant) {
        assert!(
            now.duration_since(self.last_progress) < self.inactivity
                && now.duration_since(self.start) < Duration::from_secs(300),
            "publishing did not settle after {:?}, no settle progress for {:?}",
            now.duration_since(self.start),
            now.duration_since(self.last_progress),
        );
    }
}

// The clock seam lets lifecycle regressions exercise late work without sleeping.
pub fn lifecycle<S, P: Eq + std::fmt::Debug, I>(
    state: &mut S,
    mut inspect: impl FnMut(&mut S) -> (P, bool),
    mut drive: impl FnMut(&mut S),
    mut poll: impl FnMut(&mut S) -> Vec<I>,
    mut apply: impl FnMut(&mut S, I),
    mut now: impl FnMut() -> Instant,
) {
    let start = now();
    let mut last_progress = start;
    let mut phase = inspect(state).0;
    loop {
        if inspect(state).1 {
            return;
        }
        drive(state);
        for input in poll(state) {
            apply(state, input);
            last_progress = now();
        }
        let next = inspect(state).0;
        if next != phase {
            phase = next;
            last_progress = now();
        }
        let current = now();
        assert!(
            current.duration_since(last_progress) < Duration::from_secs(90)
                && current.duration_since(start) < Duration::from_secs(300),
            "publishing lifecycle stalled; last phase={phase:?}"
        );
        std::thread::yield_now();
    }
}
