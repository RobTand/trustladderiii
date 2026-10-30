use std::time::SystemTime;

/// Wall-clock evidence for liveness and allowance probes. Time alone is never
/// evidence that an upstream attempt has failed or is absent.
pub trait Clock {
    fn now(&self) -> SystemTime;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}
