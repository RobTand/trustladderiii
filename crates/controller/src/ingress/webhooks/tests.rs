use super::*;
use hmac::{Hmac, Mac};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use crate::store_support as database;

fn ping(address: SocketAddr, id: usize) {
    let body = b"{}";
    let mut mac = Hmac::<Sha256>::new_from_slice(b"test-secret").unwrap();
    mac.update(body);
    let signature = hex::encode(mac.finalize().into_bytes());
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(stream, "POST /github/events HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 2\r\nX-GitHub-Event: ping\r\nX-GitHub-Delivery: pending-{id}\r\nX-Hub-Signature-256: sha256={signature}\r\n\r\n{{}}").unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 202"), "{reply}");
}

pub(super) fn bootstrap() -> Bootstrap {
    Bootstrap {
        repos: vec![],
        rob: 1,
        operations: None,
        slots: 1,
        pool: 1,
    }
}

#[test]
fn pending_wakes_are_coalesced_before_source_run() {
    let mut database = database::Database::start();
    let settings = database.create("pending_wakes");
    let source = Webhooks::bind(
        "127.0.0.1:0".parse().unwrap(),
        Store::open(&settings).unwrap(),
        bootstrap(),
        b"test-secret".to_vec(),
        None,
    )
    .unwrap();
    for id in 0..32 {
        ping(source.address(), id);
    }
    assert_eq!(
        Store::open(&settings)
            .unwrap()
            .pending_ingress()
            .unwrap()
            .len(),
        32
    );
    source.control.stop();
    let (sender, receiver) = mpsc::channel();
    source.run(sender).unwrap();
    assert_eq!(
        receiver.try_iter().count(),
        1,
        "pre-registration deliveries must occupy one wake slot"
    );
    database.stop();
}

struct PanicOnce {
    calls: AtomicUsize,
    entered: Sender<usize>,
    release: Mutex<mpsc::Receiver<()>>,
}
impl Fetcher for PanicOnce {
    fn fetch(
        &self,
        _since: DateTime<Utc>,
        _snapshot: tl3_store::Snapshot,
        _bootstrap: &Bootstrap,
    ) -> Result<CatchUpBatch> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.send(call).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        if call == 0 {
            panic!("fixture fetch panic");
        }
        Ok(CatchUpBatch {
            deliveries: vec![],
            cursor: Some(Utc::now()),
        })
    }
}

#[test]
fn fetch_panic_returns_reader_and_receipt_retries() {
    let mut database = database::Database::start();
    let settings = database.create("fetch_panic");
    let (entered, calls) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let source = Webhooks::bind_with_fetcher(
        Store::open(&settings).unwrap(),
        Arc::new(PanicOnce {
            calls: AtomicUsize::new(0),
            entered,
            release: Mutex::new(blocked),
        }),
    )
    .unwrap();
    let address = source.address();
    let control = source.control();
    let (sender, wakes) = mpsc::channel();
    let running = thread::spawn(move || source.run(sender));
    assert_eq!(calls.recv_timeout(Duration::from_secs(10)).unwrap(), 0);
    release.send(()).unwrap();
    assert!(
        calls.recv_timeout(Duration::from_millis(250)).is_err(),
        "a panic must not start a timed retry"
    );
    control.gap();
    assert_eq!(calls.recv_timeout(Duration::from_secs(10)).unwrap(), 1);
    ping(address, 1);
    wakes.recv_timeout(Duration::from_secs(10)).unwrap();
    release.send(()).unwrap();
    wakes.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        Store::open(&settings)
            .unwrap()
            .ingress_cursor("github-catch-up")
            .unwrap()
            .is_some()
    );
    control.stop();
    running.join().unwrap().unwrap();
    database.stop();
}
