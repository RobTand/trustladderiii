//! One LISTEN session per console. Every notification marks the board stale.
//! A lost session reconnects after a bounded backoff, then marks the board
//! stale, since changes may have committed while it was away.

use crate::rebuild::Rebuilds;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

/// Reconnect delays: doubling from the first, never above the cap.
pub(crate) struct Backoff {
    next: Duration,
}

impl Backoff {
    const FIRST: Duration = Duration::from_millis(100);
    const CAP: Duration = Duration::from_secs(5);

    pub(crate) fn new() -> Self {
        Self { next: Self::FIRST }
    }

    pub(crate) fn delay(&mut self) -> Duration {
        todo!()
    }

    pub(crate) fn reset(&mut self) {
        todo!()
    }
}

pub(crate) fn spawn(
    settings: tl3_config::Store,
    rebuilds: Arc<Rebuilds>,
    connected: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    let _ = (settings, rebuilds, connected, Ordering::SeqCst);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_its_cap_and_resets() {
        let mut backoff = Backoff::new();
        let delays: Vec<_> = (0..9).map(|_| backoff.delay().as_millis()).collect();
        assert_eq!(delays, [100, 200, 400, 800, 1600, 3200, 5000, 5000, 5000]);
        backoff.reset();
        assert_eq!(backoff.delay(), Duration::from_millis(100));
    }
}
