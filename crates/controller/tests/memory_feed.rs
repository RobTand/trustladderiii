//! #773: unavailable Hindsight holds delivery until an explicit upstream retry.
#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::json;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError},
};
use std::time::Duration;
use tl3_controller::{adapters::workers::Supervisor, executor::Executor};
use tl3_store::Store;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

const WAIT: Duration = Duration::from_secs(30);
const OPERATION: &str = "77300000-0000-0000-0000-000000000001";

struct Fixture {
    // Keep Postgres alive until the feed's sessions have ended.
    _database: support::Database,
    _scratch: tempfile::TempDir,
    runtime: tokio::runtime::Runtime,
    _server: MockServer,
    supervisor: Supervisor,
    store: Store,
    settings: tl3_config::Store,
    attempts: Receiver<bool>,
    available: Arc<AtomicBool>,
    response: Arc<(Mutex<bool>, Condvar)>,
}

impl Fixture {
    fn new(block_response: bool) -> Self {
        let mut database = support::Database::start();
        let settings = database.create("feed");
        let mut store = Store::open(&settings).unwrap();
        store
            .queue_memory(OPERATION, &json!({"content": "retained across outage"}))
            .unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let server = runtime.block_on(MockServer::start());
        let available = Arc::new(AtomicBool::new(false));
        let response = Arc::new((Mutex::new(!block_response), Condvar::new()));
        let (sender, attempts) = mpsc::channel();
        runtime.block_on(
            Mock::given(method("POST"))
                .respond_with({
                    let available = available.clone();
                    let response = response.clone();
                    move |request: &wiremock::Request| {
                        let succeeded = available.load(Ordering::SeqCst);
                        sender.send(succeeded).unwrap();
                        let (ready, changed) = &*response;
                        let ready = ready.lock().unwrap();
                        let _ready = changed.wait_while(ready, |ready| !*ready).unwrap();
                        if !succeeded {
                            return ResponseTemplate::new(503);
                        }
                        let body: serde_json::Value = request.body_json().unwrap();
                        ResponseTemplate::new(200).set_body_json(json!({
                            "success": true, "async": true, "bank_id": "tl3",
                            "items_count": 1, "operation_id": body["operation_id"],
                        }))
                    }
                })
                .mount(&server),
        );
        let mut config =
            tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        config.store = settings.clone();
        config.workers.runs = scratch.path().join("runs");
        config.hindsight.endpoint = server.uri().parse().unwrap();
        config.hindsight.api_key_file = None;
        let supervisor = Supervisor::new(&config, &scratch.path().join("records")).unwrap();
        assert!(
            !attempts.recv_timeout(WAIT).unwrap(),
            "first delivery sees the outage"
        );
        Self {
            _database: database,
            _scratch: scratch,
            runtime,
            _server: server,
            supervisor,
            store,
            settings,
            attempts,
            available,
            response,
        }
    }

    fn retry_signal(&self) {
        let mut client = postgres::Config::new()
            .host(&self.settings.host)
            .port(self.settings.port)
            .user(&self.settings.user)
            .dbname(&self.settings.database)
            .password("tl3-disposable-test-password")
            .connect(postgres::NoTls)
            .unwrap();
        client.batch_execute("NOTIFY tl3_memory_retry").unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Release a responder even when an assertion fails.
        *self.response.0.lock().unwrap() = true;
        self.response.1.notify_all();
        self.runtime.block_on(self._server.reset());
    }
}

/// #773: 64 ordinary drains in an outage window must not retry the feed.
#[test]
fn retirement_wakes_do_not_retry_unavailable_hindsight() {
    let mut fixture = Fixture::new(false);
    for _ in 0..64 {
        fixture.supervisor.retire(&mut fixture.store).unwrap();
    }
    assert_eq!(
        fixture.attempts.recv_timeout(Duration::from_secs(1)),
        Err(RecvTimeoutError::Timeout),
        "Hindsight outage holds the feed instead of retrying on each drain"
    );
    assert_eq!(fixture.store.pending_memories(0).unwrap().len(), 1);
}

/// #773: a recovery signal racing the failed response is retained, without a drain or timer.
#[test]
fn recovery_during_failed_delivery_resumes_without_a_controller_wake() {
    let fixture = Fixture::new(true);
    fixture.available.store(true, Ordering::SeqCst);
    fixture.retry_signal();
    *fixture.response.0.lock().unwrap() = true;
    fixture.response.1.notify_all();
    assert_eq!(
        fixture.attempts.recv_timeout(WAIT),
        Ok(true),
        "Hindsight recovery must release the hold, including an in-flight failure"
    );
}
