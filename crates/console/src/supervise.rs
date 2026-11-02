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
        // Dropped on return and while unwinding from a panic alike.
        struct Report(mpsc::Sender<&'static str>, &'static str);
        impl Drop for Report {
            fn drop(&mut self) {
                let _ = self.0.send(self.1);
            }
        }
        let report = Report(self.stopped.clone(), name);
        thread::Builder::new()
            .name(format!("tl3-console-{name}"))
            .spawn(move || {
                let _report = report;
                body();
            })?;
        Ok(())
    }

    /// Block until any supervised thread stops; returns its name.
    pub(crate) fn wait(self) -> &'static str {
        self.first
            .recv()
            .expect("the supervisor holds a sender until it returns")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panicking_thread_ends_the_wait() {
        let supervisor = Supervisor::new();
        supervisor.spawn("http", thread::park).unwrap();
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
