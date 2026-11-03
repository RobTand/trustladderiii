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
