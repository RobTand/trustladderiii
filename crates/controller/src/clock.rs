use std::time::SystemTime;

/// Wall-clock evidence for liveness and allowance probes. Time alone is never
/// evidence that an upstream attempt has failed or is absent.
pub trait Clock {
    fn now(&self) -> SystemTime;

    /// Entropy for a fresh database; test clocks may pin the initialization identity.
    fn new_world_id(&self) -> std::io::Result<[u8; 16]> {
        use std::io::Read;
        let mut bytes = [0; 16];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(bytes)
    }
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}
