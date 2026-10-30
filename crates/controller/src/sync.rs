#[cfg(loom)]
pub(crate) use loom::sync::{Arc, Mutex};
#[cfg(loom)]
pub(crate) use loom::thread;
#[cfg(not(loom))]
pub(crate) use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
};

// Loom 0.7 records a send even after its receiver was dropped. Serialize that
// boundary so failed sends have std's semantics and cannot leak model messages.
#[cfg(loom)]
pub(crate) mod mpsc {
    use loom::sync::{Arc, Mutex, mpsc as channel};
    use std::sync::mpsc::{RecvError, RecvTimeoutError, SendError, TryRecvError};

    pub struct Sender<T> {
        inner: channel::Sender<T>,
        alive: Arc<Mutex<bool>>,
    }
    pub struct Receiver<T> {
        inner: Option<channel::Receiver<T>>,
        alive: Arc<Mutex<bool>>,
    }
    pub(crate) fn channel<T>() -> (Sender<T>, Receiver<T>) {
        let (sender, receiver) = channel::channel();
        let alive = Arc::new(Mutex::new(true));
        (
            Sender {
                inner: sender,
                alive: alive.clone(),
            },
            Receiver {
                inner: Some(receiver),
                alive,
            },
        )
    }
    impl<T> Clone for Sender<T> {
        fn clone(&self) -> Self {
            Self {
                inner: self.inner.clone(),
                alive: self.alive.clone(),
            }
        }
    }
    impl<T> Sender<T> {
        pub(crate) fn send(&self, message: T) -> Result<(), SendError<T>> {
            let alive = self.alive.lock().unwrap();
            if *alive {
                self.inner.send(message)
            } else {
                Err(SendError(message))
            }
        }
    }
    impl<T> Receiver<T> {
        pub(crate) fn recv(&self) -> Result<T, RecvError> {
            self.inner.as_ref().unwrap().recv()
        }
        pub(crate) fn recv_timeout(
            &self,
            timeout: std::time::Duration,
        ) -> Result<T, RecvTimeoutError> {
            self.inner.as_ref().unwrap().recv_timeout(timeout)
        }
        pub(crate) fn try_recv(&self) -> Result<T, TryRecvError> {
            self.inner.as_ref().unwrap().try_recv()
        }
    }
    impl<T> Drop for Receiver<T> {
        fn drop(&mut self) {
            let mut alive = self.alive.lock().unwrap();
            *alive = false;
            drop(self.inner.take());
        }
    }
}

#[cfg(all(test, loom))]
mod tests {
    use super::mpsc;
    #[test]
    fn loom_dropped_receiver_does_not_retain_messages() {
        loom::model(|| {
            let (sender, receiver) = mpsc::channel();
            let other = sender.clone();
            let send = loom::thread::spawn(move || other.send(1));
            drop(receiver);
            let _ = send.join().unwrap();
            assert!(sender.send(2).is_err());
        });
    }
}
