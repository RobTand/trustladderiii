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
    store: Store,
    _database: support::Database,
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
        self.sql_database(&self.settings.database)
    }

    fn sql_database(&self, database: &str) -> postgres::Client {
        postgres::Config::new()
            .host(&self.settings.host)
            .port(self.settings.port)
            .user(&self.settings.user)
            .dbname(database)
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
fn recovery_during_failed_delivery_keeps_explicit_retry_evidence() {
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

struct IncidentRecorder(Arc<Mutex<Vec<String>>>);
impl Executor for IncidentRecorder {
    fn input_incident(
        &mut self,
        notice: &tl3_controller::input_errors::IncidentNotice,
        _: i64,
    ) -> tl3_controller::Result<()> {
        self.0.lock().unwrap().push(notice.summary.clone());
        Ok(())
    }
    fn execute(
        &mut self,
        _: &tl3_controller::executor::Delivery,
    ) -> tl3_controller::Result<tl3_controller::executor::Completion> {
        Ok(tl3_controller::executor::Completion::confirmed(vec![]))
    }
    fn reconcile(
        &mut self,
        _: &tl3_controller::executor::Delivery,
    ) -> tl3_controller::Result<tl3_controller::executor::Reconciliation> {
        Ok(tl3_controller::executor::Reconciliation::Absent)
    }
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
    let notices = Arc::new(Mutex::new(Vec::new()));
    registry.register(Upstream::GitHub, IncidentRecorder(notices.clone()));
    let mut bootstrap = Bootstrap::from_config(&fixture.config).unwrap();
    bootstrap.operations = Some(tl3_config::OperationsIssue {
        owner: "owner".into(),
        name: "operations".into(),
        installation_id: 3,
        number: 17,
    });
    let mut controller = Controller::open_with_clock(
        Store::open(&fixture.settings).unwrap(),
        registry,
        bootstrap,
        clock.clone(),
    )
    .unwrap();
    drain(&mut controller);
    assert_eq!(
        notices.lock().unwrap().len(),
        1,
        "the existing incident posting path reports the outage once"
    );
    let snapshot = controller.snapshot().unwrap();
    assert!(snapshot.rows.iter().any(|row| row.table == "incidents"
        && row.value["cause"] == "upstream"
        && row.value["input_failure"]["posted"] == true));
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

/// #773: a lost feed session is visible and retry refuses a missing consumer.
#[test]
fn feed_connection_failure_is_visible_and_retry_reports_no_consumer() {
    let mut fixture = Fixture::new(false);
    fixture.barrier();
    fixture.sql().query("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = current_database() AND (application_name = 'tl3-memory-feed' OR query LIKE 'LISTEN tl3_memory_retry%')", &[]).unwrap();
    let error = fixture.supervisor().memory_barrier().unwrap_err();
    assert!(
        error.to_string().contains("no consumer"),
        "the failure reaches supervision"
    );
    let _ = fixture.supervisor().memory_barrier();
    assert_eq!(
        fixture.store.memory_feed_state().unwrap(),
        MemoryFeedState::Failed,
        "lost feed session must expose failed state"
    );
    let (sender, receiver) = mpsc::channel();
    let mut source = fixture.supervisor().source();
    source.poll_once(&mut fixture.store, &sender).unwrap();
    assert!(
        matches!(
            receiver.recv_timeout(WAIT),
            Ok(tl3_controller::cli::Input::Wake)
        ),
        "source registration must replay an exit that already happened"
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
    fixture
        .sql_database("postgres")
        .batch_execute("ALTER DATABASE feed ALLOW_CONNECTIONS false")
        .unwrap();
    drop(fixture.supervisor.take());
    let sessions: i64 = observer.query_one("SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND (application_name = 'tl3-memory-feed' OR query LIKE 'LISTEN tl3_memory_retry%')", &[]).unwrap().get(0);
    assert_eq!(
        sessions, 0,
        "drop must join the thread and close its listener without a fresh connection"
    );
}

fn controller(fixture: &Fixture, clock: &FakeClock) -> tl3_controller::Controller {
    use tl3_controller::{Bootstrap, Controller, adapters, executor::Upstream};
    let mut registry = adapters::simulation(fixture._scratch.path()).unwrap();
    registry.register(Upstream::Workers, fixture.supervisor().clone());
    Controller::open_with_clock(
        Store::open(&fixture.settings).unwrap(),
        registry,
        Bootstrap::from_config(&fixture.config).unwrap(),
        clock.clone(),
    )
    .unwrap()
}

fn hindsight_incident(controller: &mut tl3_controller::Controller) -> tl3_store::WorldRow {
    controller
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .find(|row| {
            row.table == "incidents"
                && row.value["open"] == true
                && row.value["signature"]["class"] == "hindsight_unavailable"
        })
        .expect("an open Hindsight incident")
}

fn kill_consumer(fixture: &Fixture) {
    fixture.sql().query("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = current_database() AND application_name = 'tl3-memory-feed'", &[]).unwrap();
}

/// #773 / Q7: reusing an incident after consumer restart must rearm its probe.
#[test]
fn outage_consumer_death_and_restart_rearms_automatic_recovery() {
    let mut fixture = Fixture::new(false);
    fixture.barrier();
    let clock = FakeClock(std::rc::Rc::new(std::cell::Cell::new(1_000_000)));
    let mut ctl = controller(&fixture, &clock);
    drain(&mut ctl);
    let first = hindsight_incident(&mut ctl).id;
    kill_consumer(&fixture);
    assert!(fixture.supervisor().memory_barrier().is_err());
    drain(&mut ctl);
    assert_eq!(ctl.next_look_in().unwrap(), None);
    drop(ctl);
    drop(fixture.supervisor.take());
    fixture.supervisor =
        Some(Supervisor::new(&fixture.config, &fixture._scratch.path().join("records")).unwrap());
    assert!(!fixture.attempts.recv_timeout(WAIT).unwrap());
    fixture.barrier();
    let mut ctl = controller(&fixture, &clock);
    drain(&mut ctl);
    assert_eq!(hindsight_incident(&mut ctl).id, first);
    assert_eq!(
        ctl.next_look_in().unwrap(),
        Some(Duration::from_secs(120)),
        "a reused held incident must have an armed recovery probe"
    );
    ctl.look_now().unwrap();
    fixture.barrier();
    assert!(!fixture.attempts.recv_timeout(WAIT).unwrap());
    fixture.available.store(true, Ordering::SeqCst);
    clock.0.set(clock.0.get() + 120_000);
    ctl.look_due().unwrap();
    fixture.acknowledged();
    drain(&mut ctl);
    assert!(
        ctl.snapshot()
            .unwrap()
            .rows
            .iter()
            .all(|row| row.table != "incidents" || row.value["open"] != true)
    );
}

/// #773: thread exit wakes settled ingress, with no manual retirement call.
#[test]
fn failed_feed_wakes_its_supervisor_without_manual_retirement() {
    let mut fixture = Fixture::new(true);
    let (sender, receiver) = mpsc::channel();
    let mut source = fixture.supervisor().source();
    source.poll_once(&mut fixture.store, &sender).unwrap();
    while receiver.try_recv().is_ok() {}
    kill_consumer(&fixture);
    *fixture.response.0.lock().unwrap() = true;
    fixture.response.1.notify_all();
    assert!(fixture.supervisor().memory_barrier().is_err());
    assert!(
        matches!(
            receiver.recv_timeout(WAIT),
            Ok(tl3_controller::cli::Input::Wake)
        ),
        "feed exit must wake its settled supervisor"
    );
    assert_eq!(
        fixture.store.memory_feed_state().unwrap(),
        MemoryFeedState::Failed
    );
}

/// #773: feed failure cannot skip independent worker-session cleanup.
#[test]
fn failed_feed_still_retires_worker_sessions_and_returns_the_feed_error() {
    let mut fixture = Fixture::new(false);
    fixture.barrier();
    let session = fixture.config.workers.runs.join("sessions/773");
    std::fs::create_dir_all(&session).unwrap();
    let snapshot = fixture.store.load().unwrap();
    let mut commit =
        tl3_store::Commit::new("closed-session-773", json!({"fixture":true}), &snapshot);
    commit.rows.push(tl3_store::RowChange::Put {
        table: "nodes".into(),
        id: "773".into(),
        value: json!({"phase":{"Closed":"Done"}}),
    });
    fixture.store.commit(&commit).unwrap();
    assert!(fixture.store.can_retire_worker_session("773").unwrap());
    kill_consumer(&fixture);
    assert!(fixture.supervisor().memory_barrier().is_err());
    let result = fixture
        .supervisor
        .as_mut()
        .unwrap()
        .retire(&mut fixture.store);
    assert!(result.is_err_and(|error| error.to_string().contains("no consumer")));
    assert!(
        !session.exists(),
        "independent session cleanup must still run after feed failure"
    );
}

/// #773 / Q7: escalation follows operation UUID lineage, across incident clearances.
#[test]
fn repeated_incident_for_the_same_memory_requires_rob_and_cannot_probe_clear() {
    let mut fixture = Fixture::new(false);
    fixture.barrier();
    let clock = FakeClock(std::rc::Rc::new(std::cell::Cell::new(1_000_000)));
    let mut ctl = controller(&fixture, &clock);
    drain(&mut ctl);
    let first = hindsight_incident(&mut ctl);
    assert_eq!(first.value["needs_rob"], false);
    // Retain a closed historical incident for this still-pending operation.
    fixture.sql().execute("UPDATE tl3_world SET value = jsonb_set(value, '{open}', 'false') WHERE table_name = 'incidents' AND row_id = $1", &[&first.id]).unwrap();
    clock.0.set(clock.0.get() + 120_000);
    ctl.look_due().unwrap();
    fixture.barrier();
    drain(&mut ctl);
    let second = hindsight_incident(&mut ctl);
    assert_eq!(
        second.value["needs_rob"], true,
        "only Rob clears a later incident for the same operation lineage"
    );
    assert!(
        second.value["summary"]
            .as_str()
            .unwrap()
            .contains(&format!("clear incident {}", second.id))
    );
    fixture.available.store(true, Ordering::SeqCst);
    while fixture.attempts.try_recv().is_ok() {}
    clock.0.set(clock.0.get() + 120_000);
    ctl.look_due().unwrap();
    fixture.barrier();
    drain(&mut ctl);
    assert_eq!(hindsight_incident(&mut ctl).id, second.id);
    assert_eq!(
        fixture.store.pending_memory_count().unwrap(),
        1,
        "a later incident holds the operation until Rob clears it"
    );
    assert!(
        fixture.attempts.try_recv().is_err(),
        "a later incident cannot retry before Rob clears it"
    );
    fixture.retry_signal();
    fixture.retire();
    fixture.barrier();
    assert!(
        fixture.attempts.try_recv().is_err(),
        "an explicit memory retry cannot bypass Rob's clearance"
    );
}

/// #773 / Q7: a different operation starts its own incident lineage.
#[test]
fn a_different_memory_operation_gets_its_own_first_incident() {
    let mut fixture = Fixture::new(false);
    fixture.barrier();
    let clock = FakeClock(std::rc::Rc::new(std::cell::Cell::new(1_000_000)));
    let mut ctl = controller(&fixture, &clock);
    drain(&mut ctl);
    let first = hindsight_incident(&mut ctl);
    fixture.available.store(true, Ordering::SeqCst);
    clock.0.set(clock.0.get() + 120_000);
    ctl.look_due().unwrap();
    fixture.acknowledged();
    drain(&mut ctl);
    fixture.available.store(false, Ordering::SeqCst);
    let next = "77300000-0000-0000-0000-000000000002";
    fixture
        .store
        .queue_memory(next, &json!({"content":"another operation"}))
        .unwrap();
    fixture.retire();
    fixture.barrier();
    drain(&mut ctl);
    let second = hindsight_incident(&mut ctl);
    assert_ne!(
        first.value["input_failure"]["scope"], second.value["input_failure"]["scope"],
        "distinct operation UUIDs have distinct lineages"
    );
    assert_eq!(second.value["needs_rob"], false);
}
