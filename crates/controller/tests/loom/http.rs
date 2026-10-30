use super::*;
use loom::sync::atomic::{AtomicBool as ModelBool, AtomicUsize as ModelUsize};

#[test]
fn loom_http_stop_publishes_before_accept_wake() {
    crate::loom_tests::bounded(2, || {
        let control = Control {
            address: "127.0.0.1:9".parse().unwrap(),
            stopped: Arc::new(AtomicBool::new(false)),
        };
        let published = Arc::new(ModelUsize::new(0));
        let wake = Arc::new(ModelBool::new(false));
        let producer = {
            let control = control.clone();
            let published = published.clone();
            let wake = wake.clone();
            crate::loom_tests::spawn(move || {
                published.store(42, Ordering::Relaxed);
                control.stop_with(|| wake.store(true, Ordering::Relaxed));
            })
        };
        let mut accepts = 0;
        let incoming = std::iter::from_fn(|| {
            assert_eq!(accepts, 0, "accept slept again after stop wake");
            accepts += 1;
            while !wake.load(Ordering::Relaxed) {
                loom::thread::yield_now();
            }
            Some(Err(io::ErrorKind::ConnectionAborted.into()))
        });
        let (send, _receive) = mpsc::sync_channel(1);
        accept_connections(incoming, send, Arc::new(AtomicUsize::new(0)), &control.stopped).unwrap();
        assert_eq!(published.load(Ordering::Relaxed), 42, "stop did not acquire prior publication");
        producer.join().unwrap();
    });
}
