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
use tl3_store::Listener;

/// Reconnect delays: doubling from the first, never above the cap.
pub(crate) struct Backoff {
    next: Duration,
}

impl Backoff {
    const FIRST: Duration = Duration::from_millis(100);
    const CAP: Duration = Duration::from_secs(5);
    /// A session open this long was healthy.
    pub(crate) const HEALTHY: Duration = Duration::from_secs(30);

    pub(crate) fn new() -> Self {
        Self { next: Self::FIRST }
    }

    pub(crate) fn delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = (delay * 2).min(Self::CAP);
        delay
    }

    /// A session that dropped soon after it opened keeps backing off; only a
    /// healthy one starts the delays over.
    pub(crate) fn after_session(&mut self, lasted: Duration) {
        let _ = lasted;
        todo!()
    }

    pub(crate) fn reset(&mut self) {
        self.next = Self::FIRST;
    }
}

pub(crate) fn spawn(
    settings: tl3_config::Store,
    rebuilds: Arc<Rebuilds>,
    connected: Arc<AtomicBool>,
) -> std::io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("tl3-console-feed".into())
        .spawn(move || {
            let mut backoff = Backoff::new();
            let mut reported = false;
            loop {
                match Listener::open(&settings) {
                    Ok(mut listener) => {
                        backoff.reset();
                        reported = false;
                        connected.store(true, Ordering::SeqCst);
                        rebuilds.request();
                        while listener.wait().is_ok() {
                            rebuilds.request();
                        }
                        connected.store(false, Ordering::SeqCst);
                        eprintln!("console change feed lost; reconnecting");
                        rebuilds.request();
                    }
                    Err(error) if !reported => {
                        reported = true;
                        eprintln!("console change feed unavailable: {error}; retrying");
                    }
                    Err(_) => {}
                }
                thread::sleep(backoff.delay());
            }
        })
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

    #[test]
    fn sessions_that_drop_at_once_keep_backing_off() {
        let mut backoff = Backoff::new();
        let mut delays = vec![];
        for _ in 0..4 {
            backoff.after_session(Duration::from_millis(20));
            delays.push(backoff.delay().as_millis());
        }
        assert_eq!(delays, [100, 200, 400, 800]);
        backoff.after_session(Backoff::HEALTHY);
        assert_eq!(backoff.delay(), Duration::from_millis(100));
    }
}
