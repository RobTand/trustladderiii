//! Change notifications mark the board stale; one loop rebuilds it. Marks
//! that arrive during a rebuild collapse into one more rebuild.

use std::sync::{Condvar, Mutex};

#[derive(Default)]
pub(crate) struct Rebuilds {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    stale: bool,
    stopped: bool,
}

impl Rebuilds {
    pub(crate) fn request(&self) {
        self.state.lock().expect("rebuild state").stale = true;
        self.wake.notify_one();
    }

    #[cfg(test)]
    pub(crate) fn stop(&self) {
        self.state.lock().expect("rebuild state").stopped = true;
        self.wake.notify_all();
    }

    /// Block until the board is stale, and claim it. False once stopped.
    pub(crate) fn next(&self) -> bool {
        let mut state = self.state.lock().expect("rebuild state");
        loop {
            if state.stopped {
                return false;
            }
            if state.stale {
                state.stale = false;
                return true;
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
                while rebuilds.next() {
                    count += 1;
                    started.send(count).unwrap();
                    gate.recv().unwrap();
                }
                count
            })
        };
        rebuilds.request();
        assert_eq!(builds.recv().unwrap(), 1);
        for _ in 0..10 {
            rebuilds.request();
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
        rebuilds.request();
        rebuilds.request();
        assert!(rebuilds.next());
        rebuilds.stop();
        assert!(!rebuilds.next());
    }
}
