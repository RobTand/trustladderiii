//! #817 / Q7 (found by: live probe, PB #2819 request 367 `effect:367:4` and
//! PB #2821 request 369 `effect:369:4`; ops issue #865): a maker's recorded
//! attempt whose processes are gone, with progress on stdout, a Pending
//! supervisor record and no `exit.json`, is an infrastructure failure. The real
//! supervisor, its event source and the real store run as tl3d runs them,
//! including stops and starts. A running tl3d, and every later start, must
//! reclaim the run in bounded work: a Q7 retry in a new generation, then an
//! incident on the second loss. It must never hold the request's slot silently.
//!
//! The recorded files mirror ops/fixtures/817-pb2819 (no credentials file was
//! copied or reproduced). Each recorded process identity names a real process
//! on this host and boot, which the test kills at the shape's point of death.
use super::support;
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    process::{Child, Command},
    rc::Rc,
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Ownership, Repository,
    adapters::{
        self,
        workers::{Supervisor, WorkerOutputSource},
    },
    cli::Input,
    executor::{Completion, Delivery, Executor, Look, Output, Reconciliation, Upstream},
    status,
};
use tl3_kernel::{EffectKind, Phase, Role};
use tl3_store::{Reader, Store};

/// PB #2819 stdout held 21 `progress` events and no reply.
const PROGRESS_EVENTS: usize = 21;
/// Recorded facts of `effect:367:4`, verbatim from the run directory.
const FACTS: &str = r#"{"started_ms": 1793912489728, "finished_ms": null, "outcome": "running", "release": "18e72ac5c271928e", "model": "gpt-6.1-sol", "input_tokens": 100701, "cached_tokens": 83328, "output_tokens": 1376, "usage_complete": true, "trace_id": "tr-5184c58ef4e8725830af1290c39d296e", "session_id": "tl3-367-5-1", "failure": null}"#;

#[derive(Default)]
struct Launches {
    keys: Vec<String>,
    live: Vec<Child>,
}

impl Launches {
    /// The launched workers die while tl3d is stopped; nothing records an exit.
    fn kill(&mut self) {
        for mut child in self.live.drain(..) {
            child.kill().unwrap();
            child.wait().unwrap();
        }
    }
}

impl Drop for Launches {
    fn drop(&mut self) {
        for child in &mut self.live {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Maker starts record the attempt a real supervisor launch leaves behind;
/// every other read and ending of a maker run goes to the real supervisor.
struct Orphans {
    real: Supervisor,
    fake: adapters::workers::Fake,
    runs: PathBuf,
    launches: Rc<RefCell<Launches>>,
}

fn is_maker(delivery: &Delivery) -> bool {
    matches!(
        delivery.output,
        Output::Effect {
            kind: EffectKind::StartRun(Role::Make) | EffectKind::StopRun,
            ..
        }
    )
}

impl Executor for Orphans {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        if matches!(
            delivery.output,
            Output::Effect {
                kind: EffectKind::StartRun(Role::Make),
                ..
            }
        ) {
            let child = record_attempt(&self.runs, delivery);
            let mut launches = self.launches.borrow_mut();
            launches.keys.push(delivery.key.clone());
            launches.live.push(child);
            return Ok(Completion::confirmed(vec![]));
        }
        if is_maker(delivery) {
            self.real.execute(delivery)
        } else {
            self.fake.execute(delivery)
        }
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        if is_maker(delivery) {
            self.real.reconcile(delivery)
        } else {
            self.fake.reconcile(delivery)
        }
    }
    fn look(&mut self, delivery: &Delivery) -> tl3_controller::Result<Look> {
        if is_maker(delivery) {
            self.real.look(delivery)
        } else {
            self.fake.look(delivery)
        }
    }
    fn stop_stalled(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        self.real.stop_stalled(delivery)
    }
    fn retire(&mut self, store: &mut Store) -> tl3_controller::Result<()> {
        self.real.retire(store)
    }
    fn configured_model(&self, model: &str) -> bool {
        self.real.configured_model(model)
    }
    fn probe_memory(&mut self) -> tl3_controller::Result<()> {
        self.real.probe_memory()
    }
    fn probe(&mut self, delivery: &Delivery) -> tl3_controller::Result<bool> {
        self.real.probe(delivery)
    }
    fn resume_after_quota(
        &mut self,
        delivery: &Delivery,
    ) -> tl3_controller::Result<Option<Completion>> {
        self.real.resume_after_quota(delivery)
    }
    fn quota_candidates(
        &mut self,
        delivery: &Delivery,
    ) -> tl3_controller::Result<Vec<tl3_controller::looks::QuotaModel>> {
        self.real.quota_candidates(delivery)
    }
    fn quota_model_eligible(
        &mut self,
        delivery: &Delivery,
        model: &str,
    ) -> tl3_controller::Result<bool> {
        self.real.quota_model_eligible(delivery, model)
    }
    fn quota_resume_ms(
        &mut self,
        delivery: &Delivery,
        announced: u64,
        now: u64,
    ) -> tl3_controller::Result<u64> {
        self.real.quota_resume_ms(delivery, announced, now)
    }
    fn dispatch_evidence(
        &mut self,
        delivery: &Delivery,
    ) -> tl3_controller::Result<(tl3_controller::dispatch_liveness::ProcessEvidence, bool)> {
        if is_maker(delivery) {
            self.real.dispatch_evidence(delivery)
        } else {
            self.fake.dispatch_evidence(delivery)
        }
    }
}

fn identity(pid: u32, start: u64) -> serde_json::Value {
    let read = |path: &str| fs::read_to_string(path).unwrap().trim().to_owned();
    json!({"pid": pid, "machine": read("/etc/machine-id"),
        "start": start, "boot": read("/proc/sys/kernel/random/boot_id")})
}

/// A live process's start time, in the same /proc field the supervisor checks.
fn start_time(pid: u32) -> u64 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    stat.rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap()
}

fn run_dir(runs: &Path, key: &str) -> PathBuf {
    runs.join(key.bytes().map(|b| format!("{b:02x}")).collect::<String>())
        .join("attempt-00000000")
}

/// The files a launched maker attempt had written by its last progress event.
/// The returned process stands in for both the supervisor and the worker: once
/// the test kills it, both recorded identities name an ended process on this
/// boot, and nothing writes `exit.json`.
fn record_attempt(runs: &Path, delivery: &Delivery) -> Child {
    let attempt = run_dir(runs, &delivery.key);
    fs::create_dir_all(&attempt).unwrap();
    let child = Command::new("sleep").arg("3600").spawn().unwrap();
    let identity = identity(child.id(), start_time(child.id()));
    let request = json!({"role": "maker", "model": {"name": "gpt-6.1-sol"},
        "operation": {"node": delivery.node().0, "generation": delivery.generation}});
    let mut stdout = String::new();
    for index in 0..PROGRESS_EVENTS {
        let unit = if index % 2 == 0 {
            "model_turn"
        } else {
            "tool_result"
        };
        stdout.push_str(&json!({"event": "progress", "unit": unit}).to_string());
        stdout.push('\n');
    }
    for (name, contents) in [
        ("request.json", request.to_string()),
        ("command.json", r#"["-m","tl3_workers","run"]"#.to_owned()),
        (
            "launch.json",
            json!({"python": "/usr/bin/python3"}).to_string(),
        ),
        ("launcher.log", String::new()),
        ("claim.json", identity.to_string()),
        ("stderr.log", String::new()),
        ("process.json", identity.to_string()),
        (
            "worker-started.json",
            json!({"started_ms": 1793912487965_u64}).to_string(),
        ),
        ("facts.json", FACTS.to_owned()),
        ("stdout.jsonl", stdout),
    ] {
        fs::write(attempt.join(name), contents).unwrap();
    }
    child
}

struct Fixture {
    _db: support::Database,
    settings: tl3_config::Store,
    root: tempfile::TempDir,
    config: tl3_config::Config,
    launches: Rc<RefCell<Launches>>,
    clock: Clock,
}

#[derive(Clone)]
struct Clock(Rc<Cell<u64>>);
impl tl3_controller::clock::Clock for Clock {
    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.0.get())
    }
}

/// One tl3d lifetime: startup inspection, the controller and its worker source.
struct Daemon {
    controller: Controller,
    source: WorkerOutputSource,
    store: Store,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let mut db = support::Database::start();
        let settings = db.create(name);
        let root = tempfile::tempdir().unwrap();
        let mut config =
            tl3_config::Config::parse(include_str!("../../../config/example.toml")).unwrap();
        config.store = settings.clone();
        config.workers.python = "/usr/bin/python3".into();
        config.workers.runs = root.path().join("runs");
        config.models.key_file = root.path().join("fixture-key");
        fs::write(&config.models.key_file, "fixture-only").unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        Self {
            _db: db,
            settings,
            root,
            config,
            launches: Rc::default(),
            clock: Clock(Rc::new(Cell::new(now))),
        }
    }

    /// Start tl3d in production order: claim ownership, inspect recorded runs
    /// through the supervisor, open the controller, then poll the source.
    fn start(&self) -> Daemon {
        let mut ownership = Ownership::claim(Store::open(&self.settings).unwrap()).unwrap();
        let supervisor = Supervisor::new(
            &self.config,
            &self.root.path().join("records"),
            &mut ownership,
        )
        .unwrap();
        let source = supervisor.source();
        let mut registry = adapters::simulation(&self.root.path().join("fakes")).unwrap();
        registry.register(
            Upstream::Workers,
            Orphans {
                real: supervisor,
                fake: adapters::workers::Fake::new(&self.root.path().join("fakes")),
                runs: self.config.workers.runs.clone(),
                launches: self.launches.clone(),
            },
        );
        let mut controller = Controller::open_with_clock(
            ownership.into_store(),
            registry,
            Bootstrap {
                repos: vec![Repository {
                    owner: "owner".into(),
                    name: "repo".into(),
                    installation: 1,
                }],
                operations: None,
                rob: 42,
                slots: 2,
                pool: 2,
                fail_fast: true,
            },
            self.clock.clone(),
        )
        .unwrap();
        controller.set_allowances(self.config.allowances.clone());
        Daemon {
            controller,
            source,
            store: Store::open(&self.settings).unwrap(),
        }
    }

    /// Stop tl3d and wait until its ownership lock is free for the next start.
    fn stop(&self, daemon: Daemon) {
        drop(daemon);
        let mut sql = postgres::Config::new()
            .host(&self.settings.host)
            .port(self.settings.port)
            .user(&self.settings.user)
            .dbname(&self.settings.database)
            .password("tl3-disposable-test-password")
            .connect(postgres::NoTls)
            .unwrap();
        sql.batch_execute("SET lock_timeout = '180s'").unwrap();
        sql.query_one("SELECT pg_advisory_lock($1)", &[&0x544c33434f4e5452_i64])
            .unwrap();
        sql.query_one("SELECT pg_advisory_unlock($1)", &[&0x544c33434f4e5452_i64])
            .unwrap();
    }
}

/// What a request shows after one start: its phase, its infrastructure
/// failures, its generation and its open incidents.
#[derive(Debug)]
struct Seen {
    phase: Phase,
    failures: u32,
    generation: u64,
    incidents: usize,
}

impl Daemon {
    fn seen(&mut self, settings: &tl3_config::Store) -> Seen {
        let node = self.controller.world().unwrap().nodes[0];
        let visible = status::load(&mut Reader::open(settings).unwrap()).unwrap();
        Seen {
            phase: node.phase,
            failures: u32::from(node.run_failures),
            generation: u64::from(node.generation),
            incidents: visible.requests[0].details.open_incidents.len(),
        }
    }

    /// Bounded controller work for one start: source events, a drained drive
    /// loop, the looks that are due, and the drive loop again.
    fn settle(&mut self) {
        let (tx, rx) = mpsc::channel();
        self.source.poll_once(&mut self.store, &tx).unwrap();
        for input in rx.try_iter() {
            if let Input::Observation { key, observation } = input {
                self.controller.observe(&key, observation).unwrap();
            }
        }
        self.drain();
        self.controller.look_due().unwrap();
        self.drain();
    }

    fn drain(&mut self) {
        for _ in 0..300 {
            if !self.controller.drive_once().unwrap() {
                return;
            }
        }
        panic!("controller did not settle");
    }
}

/// The recorded inputs, so a run log shows which ending reclaimed the maker.
fn print_events(settings: &tl3_config::Store, label: &str) {
    let mut reader = Reader::open(settings).unwrap();
    let mut tx = reader.transaction().unwrap();
    for row in tx
        .query(
            "SELECT event_key, payload FROM tl3_events ORDER BY revision DESC LIMIT 12",
            &[],
        )
        .unwrap()
        .into_iter()
        .rev()
    {
        let payload: serde_json::Value = row.get(1);
        let text = payload.to_string();
        println!(
            "{label}: {} {}",
            row.get::<_, String>(0),
            &text[..text.len().min(400)]
        );
    }
}

fn intake(daemon: &mut Daemon, number: u64) {
    daemon
        .controller
        .intake(LabelledIssue {
            delivery: format!("label-{number}"),
            owner: "owner".into(),
            repo: "repo".into(),
            installation: 1,
            number,
            title: "Protocol declaration reader misses structural pattern bindings".into(),
            actor: 42,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    daemon.drain();
}

/// PB #2819: the maker wrote its progress, then its processes ended with no
/// exit record while tl3d kept running, and tl3d later restarted several times.
/// The running tl3d must reclaim it on the process-exit wake, before any
/// allowance clock moves: a Q7 LostWorker infrastructure failure and a retry
/// in a new generation, which releases and re-grants the slot. The
/// retry's loss across a restart raises the incident, and later restarts keep
/// that one incident without launching again. It is never a silent wait.
#[test]
fn pb2819_dead_maker_without_exit_is_reclaimed_across_restarts() {
    let fixture = Fixture::new("pb2819_dead_maker");
    let mut daemon = fixture.start();
    intake(&mut daemon, 2819);
    daemon.settle();
    let launched = daemon.seen(&fixture.settings);
    assert_eq!(launched.phase, Phase::Making);
    assert_eq!(launched.failures, 0, "a live maker is not a failure");
    // The maker dies after its progress while this tl3d runs. Its ending is
    // evidence (the process exit wakes the source); reclaim it on that wake,
    // without waiting for an allowance clock.
    fixture.launches.borrow_mut().kill();
    daemon.settle();
    let reclaimed = daemon.seen(&fixture.settings);
    print_events(&fixture.settings, "PB #2819 reclaim");
    let launches = fixture.launches.borrow().keys.clone();
    assert_eq!(
        reclaimed.failures, 1,
        "PB #2819 shape silent at the process-exit wake: {reclaimed:?}"
    );
    assert!(reclaimed.generation > launched.generation, "{reclaimed:?}");
    assert_eq!(launches.len(), 2, "Q7 retries the lost maker once");
    assert_ne!(launches[0], launches[1], "the retry is a new operation");
    assert_eq!(reclaimed.incidents, 0, "the first loss is not an incident");
    // The retry is lost while tl3d is down; tl3d restarts across the silence.
    fixture.stop(daemon);
    fixture.launches.borrow_mut().kill();
    for restart in 1..=3 {
        fixture.clock.0.set(fixture.clock.0.get() + 5 * 3_600_000);
        let mut daemon = fixture.start();
        daemon.settle();
        let seen = daemon.seen(&fixture.settings);
        assert_eq!(
            seen.incidents, 1,
            "the retry's loss must raise one incident by restart {restart}: {seen:?}"
        );
        assert_eq!(
            fixture.launches.borrow().keys.len(),
            2,
            "no launch after the second loss"
        );
        fixture.stop(daemon);
    }
}

/// PB #2821: the maker was alive when tl3d stopped for a deploy, and gone with
/// no exit record when tl3d started again. The first start after the loss
/// reclaims it, before any allowance or later restart.
#[test]
fn pb2821_maker_lost_across_deploy_restart_is_reclaimed_at_first_start() {
    let fixture = Fixture::new("pb2821_deploy_restart");
    let mut daemon = fixture.start();
    intake(&mut daemon, 2821);
    daemon.settle();
    let launched = daemon.seen(&fixture.settings);
    assert_eq!(launched.phase, Phase::Making);
    assert_eq!(launched.failures, 0, "a live maker is not a failure");
    // Deploy: stop tl3d; the worker dies while tl3d is down; start tl3d.
    fixture.stop(daemon);
    fixture.launches.borrow_mut().kill();
    let mut daemon = fixture.start();
    daemon.settle();
    let seen = daemon.seen(&fixture.settings);
    print_events(&fixture.settings, "PB #2821 first start");
    let launches = fixture.launches.borrow().keys.len();
    assert!(
        seen.failures == 1 && seen.generation > launched.generation && launches == 2,
        "PB #2821 shape not reclaimed at the first start: {seen:?}, {launches} maker launches"
    );
    assert_eq!(
        seen.incidents, 0,
        "the first loss retries; it is not an incident"
    );
    fixture.stop(daemon);
}
