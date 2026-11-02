//! The console's threads must all keep running. If one stops or panics, the
//! process exits so systemd restarts it, rather than serving a frozen board.

use std::{sync::mpsc, thread};

pub(crate) struct Supervisor {
    stopped: mpsc::Sender<&'static str>,
    first: mpsc::Receiver<&'static str>,
}

impl Supervisor {
    pub(crate) fn new() -> Self {
        let (stopped, first) = mpsc::channel();
        Self { stopped, first }
    }

    /// Run `body` on a named thread that must never return.
    pub(crate) fn spawn(
        &self,
        name: &'static str,
        body: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<()> {
        let _ = (name, body, thread::current());
        todo!()
    }

    /// Block until any supervised thread stops; returns its name.
    pub(crate) fn wait(self) -> &'static str {
        let _ = (&self.stopped, &self.first);
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panicking_thread_ends_the_wait() {
        let supervisor = Supervisor::new();
        supervisor.spawn("http", || thread::park()).unwrap();
        supervisor
            .spawn("board", || panic!("simulated board failure"))
            .unwrap();
        assert_eq!(supervisor.wait(), "board");
    }

    #[test]
    fn a_thread_that_returns_also_ends_the_wait() {
        let supervisor = Supervisor::new();
        supervisor.spawn("feed", || {}).unwrap();
        assert_eq!(supervisor.wait(), "feed");
    }
}
