//! Change notifications mark the board stale; one loop rebuilds it. Marks
//! that arrive during a rebuild collapse into one more rebuild, and a world
//! change covers any progress tick that arrived with it.

use std::sync::{Condvar, Mutex};

#[derive(Default)]
pub(crate) struct Rebuilds {
    state: Mutex<State>,
    wake: Condvar,
}

/// What changed: worker progress alone, or the controller's world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Change {
    Progress,
    World,
}

#[derive(Default)]
struct State {
    stale: Option<Change>,
    stopped: bool,
}

impl Rebuilds {
    pub(crate) fn request(&self, change: Change) {
        let mut state = self.state.lock().expect("rebuild state");
        state.stale = state.stale.max(Some(change));
        self.wake.notify_one();
    }

    #[cfg(test)]
    pub(crate) fn stop(&self) {
        self.state.lock().expect("rebuild state").stopped = true;
        self.wake.notify_all();
    }

    /// Block until the board is stale, and claim what changed. None once stopped.
    pub(crate) fn next(&self) -> Option<Change> {
        let mut state = self.state.lock().expect("rebuild state");
        loop {
            if state.stopped {
                return None;
            }
            if let Some(change) = state.stale.take() {
                return Some(change);
            }
            state = self.wake.wait(state).expect("rebuild state");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, mpsc},
        thread,
    };

    #[test]
    fn requests_during_a_rebuild_collapse_into_one_more() {
        let rebuilds = Arc::new(Rebuilds::default());
        let (started, builds) = mpsc::channel();
        let (release, gate) = mpsc::channel::<()>();
        let worker = {
            let rebuilds = rebuilds.clone();
            thread::spawn(move || {
                let mut count = 0;
                while rebuilds.next().is_some() {
                    count += 1;
                    started.send(count).unwrap();
                    gate.recv().unwrap();
                }
                count
            })
        };
        rebuilds.request(Change::World);
        assert_eq!(builds.recv().unwrap(), 1);
        for _ in 0..10 {
            rebuilds.request(Change::Progress);
        }
        release.send(()).unwrap();
        assert_eq!(builds.recv().unwrap(), 2);
        release.send(()).unwrap();
        rebuilds.stop();
        assert_eq!(worker.join().unwrap(), 2);
        assert!(builds.try_recv().is_err());
    }

    #[test]
    fn repeated_requests_build_once_and_stop_ends_the_loop() {
        let rebuilds = Rebuilds::default();
        rebuilds.request(Change::World);
        rebuilds.request(Change::World);
        assert_eq!(rebuilds.next(), Some(Change::World));
        rebuilds.stop();
        assert_eq!(rebuilds.next(), None);
    }

    #[test]
    fn a_world_change_covers_progress_ticks() {
        let rebuilds = Rebuilds::default();
        rebuilds.request(Change::Progress);
        assert_eq!(rebuilds.next(), Some(Change::Progress));
        rebuilds.request(Change::Progress);
        rebuilds.request(Change::World);
        rebuilds.request(Change::Progress);
        assert_eq!(rebuilds.next(), Some(Change::World));
    }
}
