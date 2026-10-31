use super::*;
use loom::sync::atomic::{AtomicBool as ModelBool, AtomicUsize as ModelUsize};

#[test]
fn loom_http_stop_publishes_before_accept_wake() {
    crate::loom_tests::bounded(2, || {
        let wake = Arc::new(ModelBool::new(false));
        let signal = wake.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop_at_wake = stopped.clone();
        let control = Control {
            address: "127.0.0.1:9".parse().unwrap(),
            stopped,
            model_wake: Some(Arc::new(move || {
                assert!(
                    stop_at_wake.load(Ordering::Relaxed),
                    "stop wake before publication"
                );
                signal.store(true, Ordering::Relaxed);
            })),
        };
        let published = Arc::new(ModelUsize::new(0));
        let producer = {
            let control = control.clone();
            let published = published.clone();
            crate::loom_tests::spawn(move || {
                published.store(42, Ordering::Relaxed);
                control.stop();
            })
        };
        let mut accepts = 0;
        let incoming = std::iter::from_fn(|| {
            assert_eq!(accepts, 0, "accept slept again after stop wake");
            accepts += 1;
            while !wake.load(Ordering::Relaxed) {
                loom::thread::yield_now();
            }
            // Model the transport making the flag visible without publishing
            // earlier payload writes. The accept loop's acquire must do that.
            while !control.stopped.load(Ordering::Relaxed) {
                loom::thread::yield_now();
            }
            Some(Err(io::ErrorKind::ConnectionAborted.into()))
        });
        let (send, _receive) = mpsc::sync_channel(1);
        accept_connections(
            incoming,
            send,
            Arc::new(AtomicUsize::new(0)),
            &control.stopped,
        )
        .unwrap();
        assert_eq!(
            published.load(Ordering::Relaxed),
            42,
            "stop did not acquire prior publication"
        );
        producer.join().unwrap();
    });
}
