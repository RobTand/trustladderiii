//! #773: event-driven delivery, upstream incident recovery and owned shutdown.
#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::json;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use std::time::Duration;
use tl3_controller::{adapters::workers::Supervisor, executor::Executor};
use tl3_store::{MemoryFeedState, Store};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

const WAIT: Duration = Duration::from_secs(30);
const OPERATION: &str = "77300000-0000-0000-0000-000000000001";

struct Fixture {
    // Keep Postgres alive until the feed's sessions have ended.
    supervisor: Option<Supervisor>,
    _server: MockServer,
    runtime: tokio::runtime::Runtime,
    _scratch: tempfile::TempDir,
    _database: support::Database,
    store: Store,
    settings: tl3_config::Store,
    config: tl3_config::Config,
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
            supervisor: Some(supervisor),
            store,
            settings,
            config,
            attempts,
            available,
            response,
        }
    }

    fn supervisor(&self) -> &Supervisor {
        self.supervisor.as_ref().unwrap()
    }

    fn barrier(&self) {
        self.supervisor().memory_barrier().unwrap();
    }

    fn sql(&self) -> postgres::Client {
        postgres::Config::new()
            .host(&self.settings.host)
            .port(self.settings.port)
            .user(&self.settings.user)
            .dbname(&self.settings.database)
            .password("tl3-disposable-test-password")
            .connect(postgres::NoTls)
            .unwrap()
    }

    fn retry_signal(&mut self) {
        self.store.retry_memory_delivery().unwrap();
    }

    fn retire(&mut self) {
        self.supervisor
            .as_mut()
            .unwrap()
            .retire(&mut self.store)
            .unwrap();
    }

    fn acknowledged(&mut self) {
        self.barrier();
        assert!(
            self.store.pending_memories(0).unwrap().is_empty(),
            "acknowledged delivery removes the durable queue entry"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Release a responder even when an assertion fails.
        *self.response.0.lock().unwrap() = true;
        self.response.1.notify_all();
        drop(self.supervisor.take());
        self.runtime.block_on(self._server.reset());
    }
}

/// #773: negative assertions follow an explicit processed-event barrier.
#[test]
fn retirement_wakes_do_not_retry_unavailable_hindsight() {
    let mut fixture = Fixture::new(false);
    fixture.barrier();
    for _ in 0..64 {
        fixture.retire();
    }
    fixture.barrier();
    assert!(
        fixture.attempts.try_recv().is_err(),
        "ordinary drains cannot retry a held feed"
    );
    assert_eq!(fixture.store.pending_memories(0).unwrap().len(), 1);
    assert_eq!(
        fixture.store.memory_feed_state().unwrap(),
        MemoryFeedState::Held
    );
    fixture.available.store(true, Ordering::SeqCst);
    fixture.retry_signal();
    fixture.retire();
    fixture.acknowledged();
    assert_eq!(fixture.attempts.try_recv(), Ok(true));
}

/// #773: durable retry evidence arriving during a failed response is not lost.
#[test]
fn recovery_during_failed_delivery_resumes_without_a_controller_wake() {
    let mut fixture = Fixture::new(true);
    fixture.available.store(true, Ordering::SeqCst);
    fixture.retry_signal();
    fixture.retire();
    *fixture.response.0.lock().unwrap() = true;
    fixture.response.1.notify_all();
    fixture.acknowledged();
    assert_eq!(fixture.attempts.try_recv(), Ok(true));
}

#[derive(Clone)]
struct FakeClock(std::rc::Rc<std::cell::Cell<u64>>);
impl tl3_controller::clock::Clock for FakeClock {
    fn now(&self) -> std::time::SystemTime {
        std::time::UNIX_EPOCH + Duration::from_millis(self.0.get())
    }
}

fn drain(controller: &mut tl3_controller::Controller) {
    for _ in 0..64 {
        if !controller.drive_once().unwrap() {
            return;
        }
    }
    panic!("controller must settle");
}

/// #773 / Q51: a sanctioned probe clears the incident only after upstream acknowledgement.
#[test]
fn upstream_probe_recovers_without_operator_action_and_status_shows_hold() {
    use tl3_controller::{Bootstrap, Controller, adapters, executor::Upstream, status};
    let mut fixture = Fixture::new(false);
    fixture.barrier();
    let clock = FakeClock(std::rc::Rc::new(std::cell::Cell::new(1_000_000)));
    let mut registry = adapters::simulation(fixture._scratch.path()).unwrap();
    registry.register(Upstream::Workers, fixture.supervisor().clone());
    let bootstrap = Bootstrap::from_config(&fixture.config).unwrap();
    let mut controller = Controller::open_with_clock(
        Store::open(&fixture.settings).unwrap(),
        registry,
        bootstrap,
        clock.clone(),
    )
    .unwrap();
    drain(&mut controller);
    let mut reader = fixture.store.reader().unwrap();
    let held = status::load(&mut reader).unwrap();
    assert!(
        held.unassigned_incidents
            .iter()
            .any(|s| s.contains("Hindsight unavailable: 1 memories held")),
        "outage raises a visible upstream incident"
    );
    for rendered in [status::render_text(&held), status::render_html(&held)] {
        assert!(
            rendered.contains("Feed: held"),
            "both status views expose the hold"
        );
        assert!(rendered.contains("Pending: 1"));
        assert!(rendered.contains("Parked: 0"));
        assert!(rendered.contains("HTTP parked: 0"));
    }
    assert_eq!(
        controller.next_look_in().unwrap(),
        Some(Duration::from_secs(120))
    );
    // A failed sanctioned probe remains held; ordinary drains do not retry.
    clock.0.set(clock.0.get() + 120_000);
    controller.look_due().unwrap();
    fixture.barrier();
    drain(&mut controller);
    assert_eq!(fixture.attempts.try_recv(), Ok(false));
    fixture.available.store(true, Ordering::SeqCst);
    drain(&mut controller);
    fixture.barrier();
    assert!(fixture.attempts.try_recv().is_err());
    clock.0.set(clock.0.get() + 120_000);
    controller.look_due().unwrap();
    fixture.acknowledged();
    drain(&mut controller);
    let recovered = status::load(&mut reader).unwrap();
    assert_eq!(recovered.memory.pending, 0);
    assert!(
        recovered.unassigned_incidents.is_empty(),
        "acknowledgement clears the incident through the probe path"
    );
    assert_eq!(controller.next_look_in().unwrap(), None);
    drop(controller);
}

/// #773: a dead consumer must never make the operator retry command report success.
#[test]
fn retry_without_a_consumer_is_an_error() {
    let mut database = support::Database::start();
    let mut store = Store::open(&database.create("no_feed")).unwrap();
    let result = store.retry_memory_delivery();
    assert!(
        result.is_err_and(|e| e.to_string().contains("no consumer")),
        "retry must report no consumer"
    );
}

/// #773: a lost feed session becomes visible and can be recreated by explicit retry.
#[test]
fn feed_connection_failure_is_visible_and_retry_reports_no_consumer() {
    let mut fixture = Fixture::new(false);
    fixture.barrier();
    fixture.sql().query("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = current_database() AND (application_name = 'tl3-memory-feed' OR query LIKE 'LISTEN tl3_memory_retry%')", &[]).unwrap();
    fixture.retire();
    let _ = fixture.supervisor().memory_barrier();
    assert_eq!(
        fixture.store.memory_feed_state().unwrap(),
        MemoryFeedState::Failed,
        "lost feed session must expose failed state"
    );
    assert!(
        fixture
            .store
            .retry_memory_delivery()
            .is_err_and(|e| e.to_string().contains("no consumer"))
    );
    let status = tl3_controller::status::load(&mut fixture.store.reader().unwrap()).unwrap();
    for rendered in [
        tl3_controller::status::render_text(&status),
        tl3_controller::status::render_html(&status),
    ] {
        assert!(rendered.contains("Feed: failed"));
    }
}

/// #773: shutdown joins the feed even when opening another DB session is forbidden.
#[test]
fn shutdown_needs_no_new_database_connection_and_leaves_no_listener() {
    let mut fixture = Fixture::new(false);
    fixture.barrier();
    let mut observer = fixture.sql();
    observer
        .batch_execute("ALTER DATABASE feed ALLOW_CONNECTIONS false")
        .unwrap();
    drop(fixture.supervisor.take());
    let sessions: i64 = observer.query_one("SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND (application_name = 'tl3-memory-feed' OR query LIKE 'LISTEN tl3_memory_retry%')", &[]).unwrap().get(0);
    assert_eq!(
        sessions, 0,
        "drop must join the thread and close its listener without a fresh connection"
    );
}
