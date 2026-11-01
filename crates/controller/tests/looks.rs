use tl3_controller::executor::Output as Out;
#[path = "fixtures/bell/mod.rs"]
mod bell;
#[path = "../../store/tests/support/mod.rs"]
mod support;
#[path = "fixtures/waits/mod.rs"]
mod waits;
#[allow(dead_code)]
mod worktree_fixture;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    path::Path,
    rc::Rc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    clock::Clock,
    executor::{Completion, Delivery, Executor, Look, Observation, Reconciliation, Upstream},
    status,
};
use tl3_kernel::{Command, EffectKind, Event, NodeId, Phase};
use tl3_store::{EffectState, Reader, Store};

#[derive(Clone)]
struct FakeClock(Rc<Cell<u64>>);
impl FakeClock {
    fn new() -> Self {
        Self(Rc::new(Cell::new(1_000_000)))
    }
    fn advance(&self, secs: u64) {
        self.0.set(self.0.get() + secs * 1000);
    }
}
impl Clock for FakeClock {
    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.0.get())
    }
}
fn bootstrap() -> Bootstrap {
    Bootstrap {
        repos: vec![Repository {
            owner: "owner".into(),
            name: "sandbox".into(),
            installation: 3,
        }],
        rob: 42,
        slots: 1,
        pool: 1,
        operations: Some(tl3_config::OperationsIssue {
            owner: "owner".into(),
            name: "tl3".into(),
            installation_id: 3,
            number: 17,
        }),
    }
}
fn labelled() -> LabelledIssue {
    LabelledIssue {
        delivery: "label".into(),
        owner: "owner".into(),
        repo: "sandbox".into(),
        installation: 3,
        number: 12,
        title: "Fix a greeting".into(),
        actor: 42,
        label: "tl3".into(),
    }
}
fn allowances() -> tl3_config::Allowances {
    let mut a = tl3_config::Allowances::default();
    for value in [
        &mut a.agent_worker,
        &mut a.prismabuild,
        &mut a.github_mergify,
    ] {
        value.silence_secs = 1;
        value.no_progress_secs = 3;
    }
    a
}
fn drain(c: &mut Controller) {
    for _ in 0..300 {
        if !c.drive_once().unwrap() {
            return;
        }
    }
    panic!("controller did not settle");
}
fn open(
    settings: &tl3_config::Store,
    root: &Path,
    clock: &FakeClock,
    upstream: Upstream,
    adapter: impl Executor + 'static,
) -> Controller {
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(upstream, adapter);
    let mut c = Controller::open_with_clock(
        Store::open(settings).unwrap(),
        registry,
        bootstrap(),
        clock.clone(),
    )
    .unwrap();
    c.set_allowances(allowances());
    c.intake(labelled()).unwrap();
    drain(&mut c);
    c
}
#[derive(Default)]
struct LiveState {
    stopped: BTreeSet<String>,
    stops: usize,
    progress: u64,
    healthy: bool,
}
struct Live(Rc<RefCell<LiveState>>);
impl Executor for Live {
    fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
        Ok(Completion::confirmed(vec![]))
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(Reconciliation::Pending)
    }
    fn look(&mut self, d: &Delivery) -> tl3_controller::Result<Look> {
        let s = self.0.borrow();
        Ok(if s.healthy || s.stopped.contains(&d.key) {
            Look::Healthy
        } else {
            Look::Alive {
                progress: s.progress.to_string(),
            }
        })
    }
    fn stop_stalled(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        let mut s = self.0.borrow_mut();
        if s.stopped.insert(d.key.clone()) {
            s.stops += 1;
        }
        Ok(Completion::confirmed(vec![Observation::Kernel(
            Event::Reply {
                node: d.node(),
                generation: d.generation,
                reply: tl3_kernel::Reply::RunFailed,
            },
        )]))
    }
}
fn stall(c: &mut Controller, clock: &FakeClock) {
    clock.advance(1);
    c.look_due().unwrap();
    drain(c);
    clock.advance(3);
    c.look_due().unwrap();
    drain(c);
}
fn incident(c: &mut Controller) -> String {
    c.snapshot()
        .unwrap()
        .rows
        .into_iter()
        .rfind(|r| r.table == "incidents" && r.value["open"] == true)
        .unwrap()
        .id
}
fn comment(c: &mut Controller, id: &str, actor: u64, number: u64, comment_id: u64) {
    c.observe(
        &format!("comment:{comment_id}"),
        Observation::GitHubComment {
            repository: Repository {
                owner: "owner".into(),
                name: "tl3".into(),
                installation: 3,
            },
            issue: number,
            comment: tl3_github::Comment {
                id: comment_id,
                body: format!("clear incident {id}"),
                user: tl3_github::Actor {
                    id: actor,
                    login: "fixture".into(),
                },
                created_at: chrono::DateTime::UNIX_EPOCH,
                updated_at: chrono::DateTime::UNIX_EPOCH,
                issue_url: format!("https://api.github.com/repos/owner/tl3/issues/{number}"),
            },
        },
    )
    .unwrap();
    drain(c);
}
fn stalled_runs_and_incident_controls(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let state = Rc::new(RefCell::new(LiveState::default()));
    let mut c = open(
        settings,
        root.path(),
        &clock,
        Upstream::Workers,
        Live(state.clone()),
    );
    stall(&mut c, &clock);
    assert_eq!(state.borrow().stops, 1);
    assert_eq!(c.world().unwrap().nodes[0].run_failures, 1);
    stall(&mut c, &clock);
    assert_eq!(state.borrow().stops, 2);
    let node = c.world().unwrap().nodes[0];
    assert!(node.waiting_incident);
    assert!(!node.incident_needs_rob);
    let id = incident(&mut c);
    let message = c
        .effect(&format!("notice:{}", id.replace('.', ":")))
        .unwrap()
        .unwrap();
    assert_eq!(message.payload["repository"]["name"], "tl3");
    assert_eq!(message.payload["issue"], 17);
    assert_incident_request(&message.payload);
    // The fake GitHub receipt proves the incident was sent to the operations target.
    let deliveries = adapters::delivered_notes(root.path()).unwrap();
    assert!(deliveries.iter().any(|text| text.contains(&id)));
    let mut store = Store::open(settings).unwrap();
    let epoch = store.load().unwrap().epoch.get();
    store
        .claim_worker("fixture-progress", "0", epoch, 0)
        .unwrap();
    store
        .record_worker_progress("fixture-progress", 0, 10, "model_turn", None)
        .unwrap();
    let status = status::load(&mut Reader::open(settings).unwrap()).unwrap();
    assert!(
        status.requests[0]
            .details
            .last_progress
            .as_ref()
            .unwrap()
            .contains("Model turn completed")
    );
    assert_eq!(status.requests[0].details.open_incidents.len(), 1);
    clock.advance(1);
    c.look_due().unwrap();
    drain(&mut c);
    assert!(
        !c.world().unwrap().nodes[0].waiting_incident,
        "passing probe clears first incident"
    );
    stall(&mut c, &clock);
    stall(&mut c, &clock);
    let node = c.world().unwrap().nodes[0];
    assert!(node.waiting_incident && node.incident_needs_rob);
    let second = incident(&mut c);
    state.borrow_mut().healthy = true;
    clock.advance(1);
    c.look_due().unwrap();
    drain(&mut c);
    assert!(
        c.world().unwrap().nodes[0].waiting_incident,
        "a passing probe cannot clear a later incident"
    );
    comment(&mut c, &second, 7, 17, 1);
    assert!(c.world().unwrap().nodes[0].waiting_incident);
    comment(&mut c, &second, 42, 18, 2);
    assert!(c.world().unwrap().nodes[0].waiting_incident);
    comment(&mut c, &id, 42, 17, 3);
    assert!(
        c.world().unwrap().nodes[0].waiting_incident,
        "stale clear cannot clear a newer incident"
    );
    comment(&mut c, &second, 42, 17, 4);
    let rows = c.snapshot().unwrap().rows;
    assert!(rows.iter().any(|r| r.table == "operations_comments"));
    assert!(
        !rows.iter().any(|r| r.table == "github_comments"),
        "operations comments cannot reach the request classifier"
    );
    assert!(!c.world().unwrap().nodes[0].waiting_incident);
    assert!(
        status::load(&mut Reader::open(settings).unwrap())
            .unwrap()
            .requests[0]
            .details
            .open_incidents
            .is_empty()
    );
    assert!(c.world().unwrap().valid());
}
struct WriteDown {
    fake: adapters::workers::Fake,
    found: Rc<Cell<bool>>,
    calls: Rc<Cell<usize>>,
    quota: u8,
}
impl Executor for WriteDown {
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        if matches!(
            d.output,
            Out::Effect {
                kind: EffectKind::StartRun(tl3_kernel::Role::Ingest),
                ..
            }
        ) && !self.found.get()
        {
            self.calls.set(self.calls.get() + 1);
            if self.quota >= 3 {
                return Err(tl3_controller::Error::Quota {
                    retry_after: Duration::from_secs(if self.quota == 4 { 0 } else { 5 }),
                });
            }
            return Err(tl3_controller::Error::Invalid("fixture write outage"));
        }
        self.fake.execute(d)
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        if matches!(
            d.output,
            Out::Effect {
                kind: EffectKind::StartRun(tl3_kernel::Role::Ingest),
                ..
            }
        ) && self.found.get()
        {
            return Ok(Reconciliation::Found(Completion::confirmed(vec![
                Observation::Kernel(Event::Reply {
                    node: d.node(),
                    generation: d.generation,
                    reply: tl3_kernel::Reply::IngestDone { size: 1 },
                }),
            ])));
        }
        self.fake.reconcile(d)
    }
}
fn held(settings: &tl3_config::Store, mode: u8) {
    let root = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let found = Rc::new(Cell::new(false));
    let calls = Rc::new(Cell::new(0));
    let mut c = open(
        settings,
        root.path(),
        &clock,
        Upstream::Workers,
        WriteDown {
            fake: adapters::workers::Fake::new(root.path()),
            found: found.clone(),
            calls: calls.clone(),
            quota: mode,
        },
    );
    let key = c.snapshot().unwrap().pending_effects[0].key.clone();
    assert_eq!(calls.get(), 1);
    let before = c.snapshot().unwrap();
    for _ in 0..20 {
        assert!(!c.drive_once().unwrap());
    }
    assert_eq!(c.snapshot().unwrap(), before);
    match mode {
        0 => {
            found.set(true);
            c.look_now().unwrap();
            drain(&mut c);
            assert_eq!(
                c.effect(&key).unwrap().unwrap().state,
                EffectState::Confirmed
            );
            assert_eq!(calls.get(), 1);
        }
        1 => {
            c.observe(
                "stop",
                Observation::Kernel(Event::Command {
                    node: NodeId(0),
                    id: 1,
                    command: Command::Stop,
                }),
            )
            .unwrap();
            drain(&mut c);
            assert_eq!(
                c.effect(&key).unwrap().unwrap().state,
                EffectState::Cancelled
            );
            assert!(matches!(
                c.world().unwrap().nodes[0].phase,
                Phase::Closed(_)
            ));
        }
        2 => {
            clock.advance(1);
            c.look_due().unwrap();
            drain(&mut c);
            clock.advance(1);
            c.look_due().unwrap();
            drain(&mut c);
            assert!(c.world().unwrap().nodes[0].waiting_incident);
            assert_eq!(calls.get(), 2);
            assert!(!incident(&mut c).is_empty());
        }
        3 => {
            clock.advance(4);
            c.look_due().unwrap();
            drain(&mut c);
            assert_eq!(calls.get(), 1);
            found.set(true);
            clock.advance(1);
            c.look_due().unwrap();
            drain(&mut c);
            assert_eq!(c.world().unwrap().nodes[0].run_failures, 0);
            assert!(!c.world().unwrap().nodes[0].waiting_incident);
        }
        4 => {
            assert_eq!(c.next_look_in().unwrap(), Some(Duration::ZERO));
            found.set(true);
            c.look_due().unwrap();
            drain(&mut c);
            assert!(c.next_look_in().unwrap().is_none());
        }
        _ => unreachable!(),
    }
    assert!(c.world().unwrap().valid());
}
fn owner_progress_contract(settings: &tl3_config::Store, upstream: Upstream) {
    let root = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let state = Rc::new(RefCell::new(LiveState::default()));
    let mut c = open(settings, root.path(), &clock, upstream, Live(state.clone()));
    stall(&mut c, &clock);
    if upstream == Upstream::PrismaBuild {
        assert!(c.world().unwrap().nodes[0].job.is_some());
        assert!(!c.world().unwrap().nodes[0].waiting_incident);
        assert_eq!(state.borrow().stops, 0);
    } else {
        assert!(c.world().unwrap().nodes[0].waiting_incident);
    }
    assert!(c.world().unwrap().valid());
}
fn permanent_incident_clear(settings: &tl3_config::Store) {
    permanent_incident_clear_at(settings, false);
}
fn permanent_incident_clear_at(settings: &tl3_config::Store, request_issue: bool) {
    struct Permanent;
    impl Executor for Permanent {
        fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
            Ok(Completion {
                failure_class: None,
                progress: None,
                ack: tl3_kernel::Ack::Permanent,
                observations: vec![],
            })
        }
        fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
            Ok(Reconciliation::Absent)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let mut c = open(
        settings,
        root.path(),
        &FakeClock::new(),
        Upstream::Workers,
        Permanent,
    );
    assert!(!c.world().unwrap().nodes[0].waiting_incident);
    let id = incident(&mut c);
    comment(&mut c, &id, 7, 17, 1);
    assert_eq!(incident(&mut c), id);
    if request_issue {
        c.observe(
            "request-clear",
            Observation::GitHubComment {
                repository: bootstrap().repos[0].clone(),
                issue: 12,
                comment: tl3_github::Comment {
                    id: 200,
                    body: format!("clear incident {id}"),
                    user: tl3_github::Actor {
                        id: 42,
                        login: "fixture".into(),
                    },
                    created_at: chrono::DateTime::UNIX_EPOCH,
                    updated_at: chrono::DateTime::UNIX_EPOCH,
                    issue_url: "https://api.github.com/repos/owner/sandbox/issues/12".into(),
                },
            },
        )
        .unwrap();
        drain(&mut c);
    } else {
        comment(&mut c, &id, 42, 17, 2);
    }
    assert!(
        status::load(&mut Reader::open(settings).unwrap())
            .unwrap()
            .requests[0]
            .details
            .open_incidents
            .is_empty()
    );
    assert!(matches!(
        c.world().unwrap().nodes[0].phase,
        Phase::AwaitingRob
    ));
}

#[derive(Default)]
struct OutageState {
    reads: usize,
    writes: usize,
    probes: usize,
    recovered: bool,
    fail_probe: bool,
    fail_stop: bool,
    fail_resume: bool,
}
struct Outage {
    state: Rc<RefCell<OutageState>>,
    fake: adapters::github::Fake,
}
impl Executor for Outage {
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        if matches!(d.output, Out::Notice { .. }) {
            return self.fake.execute(d);
        }
        self.state.borrow_mut().writes += 1;
        Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into())
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into())
    }
    fn look(&mut self, _: &Delivery) -> tl3_controller::Result<Look> {
        let mut s = self.state.borrow_mut();
        s.reads += 1;
        Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into())
    }
    fn probe(&mut self, _: &Delivery) -> tl3_controller::Result<bool> {
        let mut s = self.state.borrow_mut();
        s.probes += 1;
        if s.fail_probe {
            Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into())
        } else {
            Ok(s.recovered)
        }
    }
    fn stop_stalled(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
        self.state.borrow_mut().fail_stop = true;
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied).into())
    }
    fn resume_after_quota(&mut self, _: &Delivery) -> tl3_controller::Result<Option<Completion>> {
        self.state.borrow_mut().fail_resume = true;
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied).into())
    }
}
fn read_outages_keep_looking(settings: &tl3_config::Store, fail_probe: bool) {
    let root = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let state = Rc::new(RefCell::new(OutageState {
        fail_probe,
        ..Default::default()
    }));
    let mut c = open(
        settings,
        root.path(),
        &clock,
        Upstream::GitHub,
        Outage {
            state: state.clone(),
            fake: adapters::github::Fake::new(root.path()),
        },
    );
    assert_eq!(state.borrow().writes, 1);
    for n in 0..2 {
        clock.advance(1);
        c.look_due().unwrap();
        drain(&mut c);
        assert!(c.next_look_in().unwrap().is_some());
        let incidents = c
            .snapshot()
            .unwrap()
            .rows
            .into_iter()
            .filter(|r| r.table == "incidents" && r.value["open"] == true)
            .count();
        assert_eq!(incidents, usize::from(n == 1));
    }
    let id = incident(&mut c);
    let snapshot = c.snapshot().unwrap();
    let message = c.effect(&format!("incident:{id}")).unwrap().unwrap();
    assert_incident_request(&message.payload);
    assert!(
        snapshot
            .pending_effects
            .iter()
            .any(|e| e.state == EffectState::Uncertain),
        "read failures cannot acknowledge an unknown write"
    );
    let probes = state.borrow().probes;
    clock.advance(1);
    c.look_due().unwrap();
    drain(&mut c);
    assert!(
        state.borrow().probes > probes,
        "first incident keeps probing"
    );
    assert_eq!(incident(&mut c), id);
    state.borrow_mut().recovered = true;
    state.borrow_mut().fail_probe = false;
    clock.advance(1);
    c.look_due().unwrap();
    // Don't dispatch another write until checking that the later probe cleared the incident.
    assert!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents" && r.id == id && r.value["open"] == false)
    );
    state.borrow_mut().recovered = false;
    for _ in 0..2 {
        clock.advance(1);
        c.look_due().unwrap();
        drain(&mut c);
    }
    let later = incident(&mut c);
    assert!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents" && r.id == later && r.value["needs_rob"] == true)
    );
    state.borrow_mut().recovered = true;
    clock.advance(1);
    c.look_due().unwrap();
    assert_eq!(
        incident(&mut c),
        later,
        "recovery cannot clear a later outage incident"
    );
    comment(&mut c, &later, 42, 17, 31);
    assert!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents" && r.id == later && r.value["open"] == false)
    );
    assert_eq!(
        c.next_look_in().unwrap(),
        Some(Duration::ZERO),
        "Rob's clearance re-arms the held operation"
    );
}
fn progress_releases_nothing(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let calls = Rc::new(Cell::new(0));
    let mut c = open(
        settings,
        root.path(),
        &clock,
        Upstream::Workers,
        WriteDown {
            fake: adapters::workers::Fake::new(root.path()),
            found: Rc::new(Cell::new(false)),
            calls: calls.clone(),
            quota: 0,
        },
    );
    let key = c.snapshot().unwrap().pending_effects[0].key.clone();
    for n in 0..12 {
        c.observe(
            &format!("progress:{n}"),
            Observation::Progress {
                key: key.clone(),
                checkpoint: n.to_string(),
            },
        )
        .unwrap();
        drain(&mut c);
    }
    c.observe(
        "quota",
        Observation::Quota {
            key,
            reset_ms: clock.0.get() + 1000,
        },
    )
    .unwrap();
    drain(&mut c);
    assert_eq!(
        calls.get(),
        1,
        "progress and quota cannot retry even their own upstream's held writes"
    );
    c.observe(
        "check-plan-news",
        Observation::CheckPlan {
            node: NodeId(0),
            sha: "a".repeat(40),
            run: adapters::prismabuild::CheckRun {
                repository: root.path().to_path_buf(),
                base_sha: tl3_worktrees::CommitId::new("b".repeat(40)).unwrap(),
                reviewed_sha: None,
                reproducing_unavailable: None,
                plan: vec![],
            },
        },
    )
    .unwrap();
    drain(&mut c);
    assert_eq!(calls.get(), 1, "check plans only release PrismaBuild holds");
    c.observe(
        "github-news",
        Observation::PullRequest {
            node: NodeId(0),
            number: 100,
        },
    )
    .unwrap();
    drain(&mut c);
    assert_eq!(
        calls.get(),
        1,
        "unrelated upstream news cannot release a held write"
    );
}
fn look_adapter_errors_survive(settings: &tl3_config::Store, resume: bool) {
    let root = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let state = Rc::new(RefCell::new(OutageState::default()));
    let mut c = open(
        settings,
        root.path(),
        &clock,
        Upstream::Workers,
        Live(Rc::new(RefCell::new(LiveState::default()))),
    );
    // Seed the same durable stop/quota intent that a crash would leave behind.
    let mut store = Store::open(settings).unwrap();
    let snapshot = store.load().unwrap();
    let row = snapshot
        .rows
        .iter()
        .find(|r| {
            r.table == "allowances"
                && r.value["delivery"]["output"]["Effect"]["kind"]
                    .get("StartRun")
                    .is_some()
        })
        .unwrap();
    let mut value = row.value.clone();
    value["due_ms"] = serde_json::json!(clock.0.get());
    if resume {
        value["quota_ms"] = serde_json::json!(clock.0.get());
    } else {
        value["stopping"] = serde_json::json!(true);
    }
    let mut commit = tl3_store::Commit::new(
        "fixture-intent",
        serde_json::json!({"intent":true}),
        &snapshot,
    );
    commit.rows.push(tl3_store::RowChange::Put {
        table: "allowances".into(),
        id: row.id.clone(),
        value,
    });
    store.commit(&commit).unwrap();
    drop(c);
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Workers,
        Outage {
            state: state.clone(),
            fake: adapters::github::Fake::new(root.path()),
        },
    );
    c = Controller::open_with_clock(
        Store::open(settings).unwrap(),
        registry,
        bootstrap(),
        clock.clone(),
    )
    .unwrap();
    c.set_allowances(allowances());
    c.look_due().unwrap();
    assert!(c.next_look_in().unwrap().is_some());
    clock.advance(1);
    c.look_due().unwrap();
    assert!(if resume {
        state.borrow().fail_resume
    } else {
        state.borrow().fail_stop
    });
    assert!(!incident(&mut c).is_empty());
}
struct TokenQueue {
    token: Rc<RefCell<adapters::mergify::Progress>>,
    terminal: Rc<Cell<bool>>,
}
impl TokenQueue {
    fn completion(&self, delivery: &Delivery) -> Completion {
        let mut completion = Completion::confirmed(if self.terminal.get() {
            vec![Observation::Merged {
                node: delivery.node(),
                sha: delivery.head_sha.clone().unwrap(),
                checks_passed: true,
            }]
        } else {
            vec![]
        });
        completion.progress = Some(tl3_controller::executor::ProgressToken::Mergify(
            self.token.borrow().clone(),
        ));
        completion
    }
}
impl Executor for TokenQueue {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        Ok(self.completion(delivery))
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(Reconciliation::Found(self.completion(delivery)))
    }
}
fn token_only_queue_progress(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let token = Rc::new(RefCell::new(adapters::mergify::Progress {
        merged_at: None,
        merge_commit_sha: None,
        queue_check: Some(adapters::mergify::QueueCheckProgress {
            id: 1,
            status: "in_progress".into(),
            conclusion: None,
            title: Some("Waiting".into()),
        }),
    }));
    let terminal = Rc::new(Cell::new(false));
    let mut c = open(
        settings,
        root.path(),
        &clock,
        Upstream::Mergify,
        TokenQueue {
            token: token.clone(),
            terminal: terminal.clone(),
        },
    );
    assert_eq!(c.world().unwrap().nodes[0].phase, Phase::Queued);
    let row = c
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .find(|r| r.table == "watches")
        .unwrap();
    let key = row.id;
    assert!(row.value["progress"]["current"].get("Mergify").is_some());
    clock.advance(2);
    token.borrow_mut().queue_check.as_mut().unwrap().title = Some("Next in queue".into());
    token.borrow_mut().merged_at = Some("2026-10-30T01:00:00Z".into());
    drain(&mut c);
    let progressed_at = clock.0.get();
    let allowance = c
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .find(|r| r.table == "allowances" && r.id == key)
        .unwrap()
        .value;
    assert_eq!(allowance["progress_ms"], progressed_at);
    assert_eq!(allowance["due_ms"], progressed_at + 3000);
    assert!(allowance["progress"].is_null() && allowance["found"].is_null());
    assert!(!c.drive_once().unwrap());
    let stable = c.snapshot().unwrap();
    assert!(!c.drive_once().unwrap());
    assert_eq!(
        c.snapshot().unwrap(),
        stable,
        "unchanged token writes nothing"
    );
    drop(c);
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Mergify,
        TokenQueue {
            token: token.clone(),
            terminal: terminal.clone(),
        },
    );
    c = Controller::open_with_clock(
        Store::open(settings).unwrap(),
        registry,
        bootstrap(),
        clock.clone(),
    )
    .unwrap();
    c.set_allowances(allowances());
    drain(&mut c);
    let allowance = c
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .find(|r| r.table == "allowances" && r.id == key)
        .unwrap()
        .value;
    assert_eq!(
        allowance["progress_ms"], progressed_at,
        "restart is not fresh progress"
    );
    clock.advance(1);
    c.look_due().unwrap();
    drain(&mut c);
    assert!(!c.world().unwrap().nodes[0].waiting_incident);
    clock.advance(2);
    c.look_due().unwrap();
    drain(&mut c);
    assert!(
        c.world().unwrap().nodes[0].waiting_incident,
        "token-only Found must reach the queue stall rule"
    );
    let snapshot = c.snapshot().unwrap();
    let incident = snapshot
        .rows
        .iter()
        .find(|r| r.table == "incidents" && r.value["open"] == true)
        .unwrap();
    assert_eq!(
        incident.value["signature"],
        serde_json::json!({"upstream":"Mergify", "class":"queue_unfinished"})
    );
    let summary = incident.value["summary"].as_str().unwrap();
    assert!(
        summary.contains("GitHub merged the change at 2026-10-30T01:00:00Z"),
        "{summary}"
    );
    assert!(
        summary.contains("Mergify's check \"Next in queue\" is still in progress"),
        "{summary}"
    );
    let notice = Store::open(settings)
        .unwrap()
        .effects()
        .unwrap()
        .into_iter()
        .find(|e| {
            e.key.starts_with("incident:")
                || e.payload["note"]
                    .as_str()
                    .is_some_and(|note| note.contains(summary))
        })
        .unwrap();
    assert!(notice.payload["note"].as_str().unwrap().contains(summary));
    terminal.set(true);
    drain(&mut c);
    c.look_now().unwrap();
    drain(&mut c);
    assert!(matches!(
        c.world().unwrap().nodes[0].phase,
        Phase::Closed(_)
    ));
    assert!(c.next_look_in().unwrap().is_none());
    assert!(
        !c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "watches" && r.id == key)
    );
}
fn settled_looks_are_quiet(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let mut c = Controller::open(
        Store::open(settings).unwrap(),
        adapters::simulation(root.path()).unwrap(),
        bootstrap(),
    )
    .unwrap();
    c.intake(labelled()).unwrap();
    drain(&mut c);
    c.look_now().unwrap();
    drain(&mut c);
    assert!(c.next_look_in().unwrap().is_none());
    let before = c.snapshot().unwrap();
    c.look_due().unwrap();
    assert_eq!(c.snapshot().unwrap(), before);
}
fn assert_incident_request(payload: &serde_json::Value) {
    let note = payload["note"].as_str().unwrap();
    assert!(
        note.contains("Request: `https://github.com/owner/sandbox/issues/12`"),
        "incident request address must be inert: {note}"
    );
    assert!(
        tl3_github::message::check_prose(note, &tl3_github::message::Policy::default()).is_ok(),
        "incident prose must pass the reference policy: {note}"
    );
}

fn stopped_worktree_saves_scratch(settings: &tl3_config::Store, check: &str) {
    let root = tempfile::tempdir().unwrap();
    let fixture = worktree_fixture::Fixture::create(root.path());
    let mut boot = bootstrap();
    boot.repos = vec![worktree_fixture::identity()];
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(Upstream::Worktrees, fixture.adapter());
    registry.register(
        Upstream::Workers,
        WaitingMaker {
            fake: adapters::workers::Fake::new(root.path()),
            stops: Rc::new(Cell::new(0)),
            owner: "prismabuild".into(),
            checkout: fixture
                .adapter()
                .checkout(&fixture.delivery(EffectKind::PrepareWorktree, 1)),
        },
    );
    let mut c = Controller::open(Store::open(settings).unwrap(), registry, boot).unwrap();
    c.intake(LabelledIssue {
        delivery: "dirty-worktree".into(),
        owner: "RobTand".into(),
        repo: "tl3-sandbox".into(),
        installation: 1,
        number: 42,
        title: "Fix a greeting".into(),
        actor: 42,
        label: "tl3".into(),
    })
    .unwrap();
    let delivery = fixture.delivery(EffectKind::PrepareWorktree, 1);
    let checkout = fixture.adapter().checkout(&delivery);
    drain(&mut c);
    assert!(matches!(c.world().unwrap().nodes[0].phase, Phase::Making));
    std::fs::write(checkout.join("shared.txt"), "Interrupted tracked edit.\n").unwrap();
    std::fs::write(checkout.join("unfinished.txt"), "Keep this work.\n").unwrap();
    c.observe(
        "stop-dirty-worktree",
        Observation::Kernel(Event::Command {
            node: NodeId(0),
            id: 100,
            command: Command::Stop,
        }),
    )
    .unwrap();
    drain(&mut c);
    let snapshot = c.snapshot().unwrap();
    assert!(
        matches!(
            c.world().unwrap().nodes[0].phase,
            Phase::Closed(tl3_kernel::Outcome::Stopped)
        ),
        "stop must finish without an incident"
    );
    assert!(!snapshot.rows.iter().any(|r| r.table == "incidents"));
    assert!(!checkout.exists());
    let reference = worktree_fixture::git(
        &fixture.repo,
        &["for-each-ref", "--format=%(refname)", "refs/tl3/scratch/"],
    );
    assert!(
        reference.starts_with("refs/tl3/scratch/tl3/issue-42-node-0/"),
        "{reference}"
    );
    for (file, content) in [
        ("shared.txt", "Interrupted tracked edit."),
        ("unfinished.txt", "Keep this work."),
    ] {
        assert_eq!(
            worktree_fixture::git(&fixture.repo, &["show", &format!("{reference}:{file}")]),
            content
        );
    }
    assert!(
        adapters::delivered_notes(root.path())
            .unwrap()
            .iter()
            .any(|note| note.contains(&reference)),
        "closing notice must include the saved ref"
    );
    if check == "write-once" {
        let row = snapshot
            .rows
            .iter()
            .find(|r| r.table == "saved_scratch" && r.id == reference)
            .unwrap();
        let effects = Reader::open(settings).unwrap().effects().unwrap();
        let effect = effects
            .iter()
            .find(|effect| effect.payload["output"]["Effect"]["kind"] == "RemoveWorktree")
            .unwrap();
        assert_eq!(
            row.version, effect.updated_revision,
            "scratch metadata must be written once with completion"
        );
    }
    if check == "expiry" {
        worktree_fixture::age_scratch(&fixture.repo, &reference);
        c.intake(LabelledIssue {
            delivery: "next-request".into(),
            owner: "RobTand".into(),
            repo: "tl3-sandbox".into(),
            installation: 1,
            number: 43,
            title: "Fix another greeting".into(),
            actor: 42,
            label: "tl3".into(),
        })
        .unwrap();
        drain(&mut c);
        assert!(
            worktree_fixture::git(
                &fixture.repo,
                &["for-each-ref", "--format=%(refname)", "refs/tl3/scratch/"]
            )
            .is_empty()
        );
        assert!(
            !c.snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "saved_scratch" && r.id == reference),
            "expired scratch metadata must be removed with its ref"
        );
        assert!(matches!(c.world().unwrap().nodes[1].phase, Phase::Making));
    }
    println!("test stopped_worktree_saves_scratch_{check} ... ok");
}

struct QuotaGitHub {
    fake: adapters::github::Fake,
    reconciles: Rc<Cell<usize>>,
}
impl Executor for QuotaGitHub {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        if matches!(
            delivery.output,
            Out::Effect {
                kind: EffectKind::PostApproval,
                ..
            }
        ) {
            Err(tl3_controller::Error::Quota {
                retry_after: Duration::from_secs(5),
            })
        } else {
            self.fake.execute(delivery)
        }
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        if matches!(
            delivery.output,
            Out::Effect {
                kind: EffectKind::PostApproval,
                ..
            }
        ) {
            self.reconciles.set(self.reconciles.get() + 1);
            Ok(Reconciliation::Pending)
        } else {
            self.fake.reconcile(delivery)
        }
    }
}
fn cancelling_github_respects_quota(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let reconciles = Rc::new(Cell::new(0));
    let mut c = open(
        settings,
        root.path(),
        &clock,
        Upstream::GitHub,
        QuotaGitHub {
            fake: adapters::github::Fake::new(root.path()),
            reconciles: reconciles.clone(),
        },
    );
    let key = c
        .snapshot()
        .unwrap()
        .pending_effects
        .into_iter()
        .find(|effect| effect.payload["output"]["Effect"]["kind"] == "PostApproval")
        .unwrap()
        .key;
    assert_eq!(reconciles.get(), 0);
    c.observe(
        "stop-during-github-quota",
        Observation::Kernel(Event::Command {
            node: NodeId(0),
            id: 1,
            command: Command::Stop,
        }),
    )
    .unwrap();
    drain(&mut c);
    assert_eq!(
        reconciles.get(),
        0,
        "cancelling GitHub must not read an exhausted API"
    );
    assert_eq!(
        c.effect(&key).unwrap().unwrap().state,
        EffectState::Cancelling
    );
    drop(c);
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::GitHub,
        QuotaGitHub {
            fake: adapters::github::Fake::new(root.path()),
            reconciles: reconciles.clone(),
        },
    );
    let mut c = Controller::open_with_clock(
        Store::open(settings).unwrap(),
        registry,
        bootstrap(),
        clock.clone(),
    )
    .unwrap();
    drain(&mut c);
    assert_eq!(
        reconciles.get(),
        0,
        "restart must retain GitHub's quota wait"
    );
    clock.advance(5);
    c.look_due().unwrap();
    drain(&mut c);
    assert!(
        reconciles.get() > 0,
        "the existing quota look must resume cancellation"
    );
    println!("test cancelling_github_respects_quota ... ok");
}

fn main() {
    if std::env::args().any(|arg| arg == "--cancel-github-quota") {
        let mut db = support::Database::start();
        cancelling_github_respects_quota(&db.create("cancel_github_quota"));
        db.stop();
        return;
    }
    if std::env::args().any(|s| s == "--r5") {
        let mut db = support::Database::start();
        let mut failures = 0;
        for (name, test) in [
            (
                "moving_maker_wait_keeps_registration",
                moving_maker_wait_keeps_registration as fn(&tl3_config::Store),
            ),
            (
                "successful_look_breaks_failure_streak",
                successful_look_breaks_failure_streak,
            ),
            (
                "successful_wait_read_breaks_failure_streak",
                successful_wait_read_breaks_failure_streak,
            ),
        ] {
            let settings = db.create(name);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| test(&settings)));
            println!(
                "R5 {name}: {}",
                if result.is_ok() { "PASS" } else { "FAIL" }
            );
            failures += usize::from(result.is_err());
        }
        db.stop();
        assert_eq!(failures, 0, "R5 look regressions");
        return;
    }

    if let Some(check) = std::env::args()
        .nth(1)
        .and_then(|arg| arg.strip_prefix("--scratch-metadata=").map(str::to_owned))
    {
        let mut db = support::Database::start();
        stopped_worktree_saves_scratch(&db.create("scratch_metadata"), &check);
        db.stop();
        return;
    }
    if let Some(case) = std::env::args()
        .nth(1)
        .and_then(|s| s.strip_prefix("--effect-http=").map(str::to_owned))
    {
        let mut db = support::Database::start();
        effect_http_failure(&db.create("effect_http"), &case);
        db.stop();
        return;
    }
    if std::env::args().any(|arg| arg == "--request-incident-clear") {
        let mut db = support::Database::start();
        permanent_incident_clear_at(&db.create("request_clear"), true);
        db.stop();
        return;
    }
    if std::env::args().any(|arg| arg == "--dirty-worktree") {
        let mut db = support::Database::start();
        stopped_worktree_saves_scratch(&db.create("dirty_worktree"), "saved");
        db.stop();
        return;
    }
    if let Some(case) = std::env::args()
        .nth(1)
        .and_then(|s| s.strip_prefix("--incident-references=").map(str::to_owned))
    {
        let mut db = support::Database::start();
        let settings = db.create("incident_references");
        match case.as_str() {
            "stall" => stalled_runs_and_incident_controls(&settings),
            "read" => read_outages_keep_looking(&settings, false),
            _ => panic!("unknown incident reference scenario"),
        }
        db.stop();
        return;
    }
    if let Some(case) = std::env::args()
        .nth(1)
        .and_then(|s| s.strip_prefix("--r4-").map(str::to_owned))
    {
        let mut db = support::Database::start();
        wait_subscription_retirement(&db.create("wait_retirement"), &case);
        db.stop();
        return;
    }
    // These run in the normal suite as well as the focused red action.
    if std::env::args().any(|s| s == "--r3") {
        let mut db = support::Database::start();
        let mut failed = 0;
        for name in [
            "missing_wait",
            "unknown_owner",
            "foreign_wait",
            "unreadable_wait",
            "quiet_wait",
            "github_due",
        ] {
            let settings = db.create(name);
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r3(&settings, name)))
                .is_err()
            {
                eprintln!("R3 {name}: FAIL");
                failed += 1;
            } else {
                println!("R3 {name}: PASS");
            }
        }
        db.stop();
        assert_eq!(failed, 0, "R3 regression failures");
        return;
    }
    let mut db = support::Database::start();
    cancelling_github_respects_quota(&db.create("cancel_github_quota"));
    for case in ["brief", "persistent", "forbidden", "webhooks"] {
        effect_http_failure(&db.create(&format!("effect_http_{case}")), case);
    }
    for name in [
        "missing_wait",
        "unknown_owner",
        "foreign_wait",
        "unreadable_wait",
        "quiet_wait",
        "github_due",
    ] {
        r3(&db.create(name), name);
    }
    for case in ["progress", "look", "prune"] {
        wait_subscription_retirement(&db.create(&format!("wait_retirement_{case}")), case);
        println!("test wait_subscription_removed_after_{case} ... ok");
    }
    moving_maker_wait_keeps_registration(&db.create("moving_wait"));
    successful_look_breaks_failure_streak(&db.create("failure_streak"));
    successful_wait_read_breaks_failure_streak(&db.create("wait_failure_streak"));
    worker_look_does_not_read_github_deliveries(&db.create("worker_only_look"));
    println!("test worker_look_does_not_read_github_deliveries ... ok");
    declared_maker_wait(&db.create("maker_wait_ended"), "done");
    declared_maker_wait(&db.create("maker_wait_lost"), "failed");
    println!("test declared_maker_wait_reads_live_ended_and_lost_owner_records ... ok");
    stalled_runs_and_incident_controls(&db.create("stalls"));
    println!("test stalled_runs_and_incident_controls ... ok");
    for (mode, name) in [
        "held_found",
        "held_cancelled",
        "write_outage_incident",
        "quota_reset",
        "zero_quota_reset",
    ]
    .into_iter()
    .enumerate()
    {
        held(&db.create(name), mode as u8);
        println!("test {name} ... ok");
    }
    owner_progress_contract(&db.create("pb_owner"), Upstream::PrismaBuild);
    println!("test pb_owns_progress_failure ... ok");
    owner_progress_contract(&db.create("queue_stall"), Upstream::Mergify);
    println!("test queue_stall_incident ... ok");
    stopped_worktree_saves_scratch(&db.create("dirty_worktree"), "saved");
    stopped_worktree_saves_scratch(&db.create("scratch_once"), "write-once");
    stopped_worktree_saves_scratch(&db.create("scratch_expiry"), "expiry");
    permanent_incident_clear_at(&db.create("request_clear"), true);
    permanent_incident_clear(&db.create("permanent_incident"));
    println!("test permanent_incident_clear ... ok");
    progress_releases_nothing(&db.create("progress_holds"));
    read_outages_keep_looking(&db.create("read_outage"), false);
    read_outages_keep_looking(&db.create("probe_outage"), true);
    look_adapter_errors_survive(&db.create("stop_eperm"), false);
    look_adapter_errors_survive(&db.create("resume_eperm"), true);
    settled_looks_are_quiet(&db.create("quiet_timer"));
    token_only_queue_progress(&db.create("token_only_queue"));
    db.stop();
    println!("17 allowance and incident scenarios passed; no skips");
}

struct WaitingMaker {
    fake: adapters::workers::Fake,
    stops: Rc<Cell<u32>>,
    owner: String,
    checkout: std::path::PathBuf,
}
impl WaitingMaker {
    fn maker(d: &Delivery) -> bool {
        matches!(
            d.output,
            Out::Effect {
                kind: EffectKind::StartRun(tl3_kernel::Role::Make),
                ..
            }
        )
    }
}
impl Executor for WaitingMaker {
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        if Self::maker(d) {
            Ok(Completion::confirmed(vec![]))
        } else {
            self.fake.execute(d)
        }
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        if Self::maker(d) {
            Ok(Reconciliation::Pending)
        } else {
            self.fake.reconcile(d)
        }
    }
    fn look(&mut self, d: &Delivery) -> tl3_controller::Result<Look> {
        if Self::maker(d) {
            Ok(Look::Waiting {
                wait: tl3_store::ExternalWait {
                    upstream: self.owner.clone(),
                    key: waits::key(),
                },
                checkout: self.checkout.clone(),
            })
        } else {
            Ok(match self.reconcile(d)? {
                Reconciliation::Found(c) => Look::Found(c),
                Reconciliation::Absent => Look::Absent,
                Reconciliation::Pending => Look::Pending,
            })
        }
    }
    fn stop_stalled(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        self.stops.set(self.stops.get() + 1);
        Ok(Completion::confirmed(vec![Observation::Kernel(
            Event::Reply {
                node: d.node(),
                generation: d.generation,
                reply: tl3_kernel::Reply::RunFailed,
            },
        )]))
    }
}
fn declared_maker_wait(settings: &tl3_config::Store, ending: &str) {
    let root = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let stops = Rc::new(Cell::new(0));
    let open = || {
        let mut registry = adapters::simulation(root.path()).unwrap();
        registry.register(
            Upstream::Workers,
            WaitingMaker {
                fake: adapters::workers::Fake::new(root.path()),
                stops: stops.clone(),
                owner: "prismabuild".into(),
                checkout: root.path().join("worktree"),
            },
        );
        registry.register(Upstream::PrismaBuild, waits::adapter(root.path()));
        let mut c = Controller::open_with_clock(
            Store::open(settings).unwrap(),
            registry,
            bootstrap(),
            clock.clone(),
        )
        .unwrap();
        c.set_allowances(allowances());
        c
    };
    let mut c = open();
    waits::record(root.path(), "ready");
    c.intake(labelled()).unwrap();
    drain(&mut c);
    assert_eq!(c.world().unwrap().nodes[0].phase, Phase::Making);
    c.look_now().unwrap();
    for state in ["ready", "claimed"] {
        waits::record(root.path(), state);
        clock.advance(1000);
        c.look_now().unwrap();
        assert_eq!(stops.get(), 0, "{state} is alive past no_progress_secs");
    }
    drop(c);
    let mut c = open();
    clock.advance(1000);
    c.look_due().unwrap();
    assert_eq!(stops.get(), 0, "restart retains the owner's wait");
    waits::record(root.path(), "missing");
    clock.advance(1000);
    c.look_now().unwrap();
    assert_eq!(stops.get(), 0, "missing starts the agent allowance once");
    waits::record(root.path(), ending);
    c.look_now().unwrap();
    assert_eq!(
        stops.get(),
        0,
        "the agent gets its allowance after owner termination"
    );
    drop(c);
    let mut c = open();
    clock.advance(2);
    c.look_due().unwrap();
    assert_eq!(stops.get(), 0);
    clock.advance(1);
    c.look_due().unwrap();
    assert_eq!(
        stops.get(),
        1,
        "silent maker is stopped after the ended action"
    );
}

struct DeliveryCounter {
    fake: adapters::github::Fake,
    reads: Rc<Cell<u32>>,
}
impl Executor for DeliveryCounter {
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        self.fake.execute(d)
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.fake.reconcile(d)
    }
    fn recover_deliveries(&mut self, _: u64, _: u64) -> tl3_controller::Result<Option<u64>> {
        self.reads.set(self.reads.get() + 1);
        Ok(None)
    }
}
fn worker_look_does_not_read_github_deliveries(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let reads = Rc::new(Cell::new(0));
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Workers,
        Live(Rc::new(RefCell::new(LiveState::default()))),
    );
    registry.register(
        Upstream::GitHub,
        DeliveryCounter {
            fake: adapters::github::Fake::new(root.path()),
            reads: reads.clone(),
        },
    );
    let mut c = Controller::open_with_clock(
        Store::open(settings).unwrap(),
        registry,
        bootstrap(),
        clock.clone(),
    )
    .unwrap();
    c.set_allowances(allowances());
    c.intake(labelled()).unwrap();
    drain(&mut c);
    c.look_now().unwrap();
    assert_eq!(reads.get(), 1);
    clock.advance(3);
    c.look_due().unwrap();
    assert_eq!(
        reads.get(),
        1,
        "a worker allowance must not read GitHub's delivery log"
    );
    c.look_now().unwrap();
    assert_eq!(
        reads.get(),
        2,
        "explicit gap recovery still reads deliveries"
    );
}

fn r3(settings: &tl3_config::Store, case: &str) {
    let root = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let stops = Rc::new(Cell::new(0));
    let reads = Rc::new(Cell::new(0));
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Workers,
        WaitingMaker {
            fake: adapters::workers::Fake::new(root.path()),
            stops: stops.clone(),
            checkout: root.path().join("worktree"),
            owner: if case == "unknown_owner" {
                "token_grant"
            } else {
                "prismabuild"
            }
            .into(),
        },
    );
    let bell = bell::Bell::new();
    let pb = waits::adapter_with_bell(root.path(), &bell.url);
    let source = pb.source();
    registry.register(Upstream::PrismaBuild, pb);
    registry.register(
        Upstream::GitHub,
        DeliveryCounter {
            fake: adapters::github::Fake::new(root.path()),
            reads: reads.clone(),
        },
    );
    let mut c = Controller::open_with_clock(
        Store::open(settings).unwrap(),
        registry,
        bootstrap(),
        clock.clone(),
    )
    .unwrap();
    c.set_allowances(allowances());
    waits::record(root.path(), "claimed");
    if case == "missing_wait" {
        waits::record(root.path(), "missing");
    }
    if case == "foreign_wait" {
        let file = root
            .path()
            .join(format!("queue/claimed/{}.json", waits::key()));
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        value["checkout_snapshot"]["parent"] =
            serde_json::json!(waits::bind(root.path(), &root.path().join("foreign")));
        std::fs::write(file, value.to_string()).unwrap();
    }
    if case == "unreadable_wait" {
        std::fs::write(
            root.path()
                .join(format!("queue/claimed/{}.json", waits::key())),
            "broken JSON",
        )
        .unwrap();
    }
    c.intake(labelled()).unwrap();
    drain(&mut c);
    if case == "github_due" {
        // A held GitHub write leaves a due GitHub allowance; no forced look.
        let mut snapshot = c.snapshot().unwrap();
        let mut store = Store::open(settings).unwrap();
        let mut change = tl3_store::Commit::new("github-due", serde_json::json!(null), &snapshot);
        let delivery: Delivery = serde_json::from_value(
            store
                .effects()
                .unwrap()
                .into_iter()
                .find(|e| {
                    serde_json::from_value::<Delivery>(e.payload.clone()).is_ok_and(|d| {
                        tl3_controller::executor::upstream(d.output) == Upstream::GitHub
                    })
                })
                .unwrap()
                .payload,
        )
        .unwrap();
        let mut value = snapshot
            .rows
            .iter()
            .find(|r| r.table == "allowances")
            .unwrap()
            .value
            .clone();
        value["delivery"] = serde_json::to_value(&delivery).unwrap();
        value["due_ms"] = serde_json::json!(clock.0.get());
        change.rows.push(tl3_store::RowChange::Put {
            table: "allowances".into(),
            id: delivery.key,
            value,
        });
        store.commit(&change).unwrap();
        snapshot = c.snapshot().unwrap();
        assert!(!snapshot.rows.iter().any(|r| r.table == "delivery_looks"));
        c.look_due().unwrap();
        assert_eq!(
            reads.get(),
            1,
            "a due GitHub allowance reads its delivery log"
        );
        return;
    }
    c.look_now().unwrap();
    if case == "quiet_wait" {
        assert!(
            c.next_look_in().unwrap().is_none(),
            "a claimed wait disarms the allowance instead of becoming an outage"
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let running = source.spawn(tx);
        bell.request().reply(0, serde_json::json!([]));
        let mut waiting = bell.request();
        assert!(
            waiting.line.contains(&waits::key()),
            "declared action joins the awaited set"
        );
        // Startup captures the cursor and rereads outstanding waits without a live look.
        for input in rx.try_iter() {
            tl3_controller::ingress::apply(&mut c, input).unwrap();
        }
        // Count real opens of the action record rather than mocking wait_record.
        use rustix::fs::inotify;
        let fd = inotify::init(inotify::CreateFlags::NONBLOCK).unwrap();
        inotify::add_watch(
            &fd,
            root.path()
                .join(format!("queue/claimed/{}.json", waits::key())),
            inotify::WatchFlags::OPEN,
        )
        .unwrap();
        let before = c.snapshot().unwrap();
        for _ in 0..4 {
            clock.advance(120);
            c.look_due().unwrap();
            drain(&mut c);
        }
        let mut buffer = [std::mem::MaybeUninit::<u8>::uninit(); 4096];
        let mut reader = inotify::Reader::new(&fd, &mut buffer);
        let mut opens = 0;
        while reader.next().is_ok() {
            opens += 1;
        }
        assert_eq!(opens, 0, "a claimed wait has no timed PrismaBuild reads");
        assert_eq!(
            c.snapshot().unwrap(),
            before,
            "a claimed wait has no timed store commits"
        );
        // A reset already reads the live record, so it needs no controller look.
        waiting.response.send(serde_json::json!({"epoch":"0123456789abcdef0123456789abcdef", "seq":0, "reset":true, "events":[]})).unwrap();
        waiting = bell.request();
        assert!(rx.try_recv().is_err(), "a live reset causes no look");
        assert_eq!(
            c.snapshot().unwrap(),
            before,
            "a live reset commits nothing"
        );
        // One terminal event goes through ingress and the actual PB reader.
        waits::record(root.path(), "done");
        waiting.reply(
            1,
            serde_json::json!([{"seq":1,"dir":"done","key":waits::key()}]),
        );
        let input = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(&input, tl3_controller::cli::Input::Look { .. }));
        let before_end = c.snapshot().unwrap().revision;
        tl3_controller::ingress::apply(&mut c, input).unwrap();
        assert_eq!(
            c.snapshot().unwrap().revision.get(),
            before_end.get() + 1,
            "one ending causes exactly one look commit"
        );
        assert!(rx.try_recv().is_err(), "one ending produces one look");
        assert_eq!(c.next_look_in().unwrap(), Some(Duration::from_secs(3)));
        drop(running);
        return;
    }
    clock.advance(3);
    c.look_due().unwrap();
    drain(&mut c);
    clock.advance(3);
    c.look_due().unwrap();
    drain(&mut c);
    if case == "unreadable_wait" {
        assert_eq!(stops.get(), 0, "read errors do not authorize a stop");
        assert!(
            c.snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "incidents" && r.value["open"] == true)
        );
    } else {
        assert_eq!(stops.get(), 1, "an invalid wait must not protect the maker");
        assert_eq!(
            c.world().unwrap().nodes[0].run_failures,
            1,
            "failure is checked after driving"
        );
    }
}

fn wait_subscription_retirement(settings: &tl3_config::Store, case: &str) {
    struct Maker {
        waiting: WaitingMaker,
        resumed: Rc<Cell<bool>>,
    }
    impl Executor for Maker {
        fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
            self.waiting.execute(d)
        }
        fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
            self.waiting.reconcile(d)
        }
        fn look(&mut self, d: &Delivery) -> tl3_controller::Result<Look> {
            if self.resumed.get() && WaitingMaker::maker(d) {
                Ok(Look::Alive {
                    progress: "resumed".into(),
                })
            } else {
                self.waiting.look(d)
            }
        }
    }
    let root = tempfile::tempdir().unwrap();
    let resumed = Rc::new(Cell::new(false));
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Workers,
        Maker {
            waiting: WaitingMaker {
                fake: adapters::workers::Fake::new(root.path()),
                stops: Rc::new(Cell::new(0)),
                owner: "prismabuild".into(),
                checkout: root.path().join("worktree"),
            },
            resumed: resumed.clone(),
        },
    );
    registry.register(Upstream::PrismaBuild, waits::adapter(root.path()));
    waits::record(root.path(), "claimed");
    let mut c = Controller::open_with_clock(
        Store::open(settings).unwrap(),
        registry,
        bootstrap(),
        FakeClock::new(),
    )
    .unwrap();
    c.set_allowances(allowances());
    c.intake(labelled()).unwrap();
    drain(&mut c);
    c.look_now().unwrap();
    let snapshot = c.snapshot().unwrap();
    let row = snapshot
        .rows
        .iter()
        .find(|r| r.table == "allowances" && r.value["external_wait"].is_object())
        .unwrap();
    let delivery: Delivery = serde_json::from_value(row.value["delivery"].clone()).unwrap();
    let encoded: String = delivery.key.bytes().map(|b| format!("{b:02x}")).collect();
    let path = root
        .path()
        .join(format!("records/{}/{encoded}.wait", delivery.node().0));
    assert!(
        path.is_file(),
        "the waiting maker has a durable subscription"
    );
    match case {
        "progress" => {
            c.observe(
                "own-progress",
                Observation::Progress {
                    key: delivery.key.clone(),
                    checkpoint: "own-work".into(),
                },
            )
            .unwrap();
            drain(&mut c);
        }
        "look" => {
            resumed.set(true);
            c.look_now().unwrap();
        }
        "prune" => {
            c.observe(
                "maker-ended",
                Observation::Kernel(Event::Reply {
                    node: delivery.node(),
                    generation: delivery.generation,
                    reply: tl3_kernel::Reply::RunFailed,
                }),
            )
            .unwrap();
            drain(&mut c);
            assert!(
                !c.snapshot()
                    .unwrap()
                    .rows
                    .iter()
                    .any(|r| r.table == "allowances" && r.id == delivery.key),
                "the old allowance was pruned"
            );
        }
        _ => panic!("unknown retirement case"),
    }
    assert!(
        !path.exists(),
        "{case} deletes the durable wait subscription"
    );
}

fn moving_maker_wait_keeps_registration(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let stops = Rc::new(Cell::new(0));
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Workers,
        WaitingMaker {
            fake: adapters::workers::Fake::new(root.path()),
            stops: stops.clone(),
            owner: "prismabuild".into(),
            checkout: root.path().join("worktree"),
        },
    );
    registry.register(Upstream::PrismaBuild, waits::adapter(root.path()));
    waits::record(root.path(), "claimed");
    let mut c = Controller::open_with_clock(
        Store::open(settings).unwrap(),
        registry,
        bootstrap(),
        clock.clone(),
    )
    .unwrap();
    c.set_allowances(allowances());
    c.intake(labelled()).unwrap();
    drain(&mut c);
    c.look_now().unwrap();
    let marker = root.path().join(format!(
        "queue/claimed/{}.100.host.1.abc.tombstone",
        waits::key()
    ));
    std::fs::rename(
        root.path()
            .join(format!("queue/claimed/{}.json", waits::key())),
        &marker,
    )
    .unwrap();
    for _ in 0..3 {
        clock.advance(1000);
        c.look_now().unwrap();
        let snapshot = c.snapshot().unwrap();
        let allowance = snapshot
            .rows
            .iter()
            .find(|row| {
                row.table == "allowances" && row.value["external_wait"]["key"] == waits::key()
            })
            .unwrap();
        assert_eq!(
            allowance.value["read_failures"], 0,
            "finish window must not count as a failed look"
        );
        assert_eq!(
            allowance.value["wait_ended"], false,
            "move must keep wait registered"
        );
        assert!(
            !snapshot
                .rows
                .iter()
                .any(|row| row.table == "incidents" && row.value["open"] == true),
            "finish window must not open an incident"
        );
        assert_eq!(stops.get(), 0);
    }
    waits::record(root.path(), "done");
    std::fs::remove_file(marker).unwrap();
    c.look_now().unwrap();
    assert_eq!(stops.get(), 0, "terminal starts the allowance");
}

struct RecoveringLook {
    healthy: Rc<Cell<bool>>,
}
impl Executor for RecoveringLook {
    fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
        Ok(Completion::confirmed(vec![]))
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(Reconciliation::Pending)
    }
    fn look(&mut self, _: &Delivery) -> tl3_controller::Result<Look> {
        if self.healthy.get() {
            Ok(Look::Alive {
                progress: "same".into(),
            })
        } else {
            Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into())
        }
    }
}
fn successful_look_breaks_failure_streak(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let healthy = Rc::new(Cell::new(false));
    let mut c = open(
        settings,
        root.path(),
        &clock,
        Upstream::Workers,
        RecoveringLook {
            healthy: healthy.clone(),
        },
    );
    for success in [false, true, false] {
        healthy.set(success);
        c.look_now().unwrap();
        assert!(
            !c.snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "incidents" && r.value["open"] == true),
            "nonconsecutive read failures must not open an incident"
        );
    }
    c.look_now().unwrap();
    assert_eq!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.table == "incidents" && r.value["open"] == true)
            .count(),
        1,
        "two consecutive read failures must open one incident"
    );
}

fn successful_wait_read_breaks_failure_streak(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Workers,
        WaitingMaker {
            fake: adapters::workers::Fake::new(root.path()),
            stops: Rc::new(Cell::new(0)),
            owner: "prismabuild".into(),
            checkout: root.path().join("worktree"),
        },
    );
    registry.register(Upstream::PrismaBuild, waits::adapter(root.path()));
    waits::record(root.path(), "claimed");
    let mut c =
        Controller::open_with_clock(Store::open(settings).unwrap(), registry, bootstrap(), clock)
            .unwrap();
    c.intake(labelled()).unwrap();
    drain(&mut c);
    let path = root
        .path()
        .join(format!("queue/claimed/{}.json", waits::key()));
    for success in [false, true, false] {
        if success {
            waits::record(root.path(), "claimed");
        } else {
            std::fs::write(&path, "broken JSON").unwrap();
        }
        c.look_now().unwrap();
        assert!(
            !c.snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "incidents" && r.value["open"] == true),
            "nonconsecutive wait read failures must not open an incident"
        );
    }
    c.look_now().unwrap();
    assert_eq!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.table == "incidents" && r.value["open"] == true)
            .count(),
        1,
        "two consecutive wait read failures must open one incident"
    );
}

struct HttpEffect {
    fake: adapters::github::Fake,
    calls: Rc<Cell<usize>>,
    status: u16,
    brief: bool,
}
impl Executor for HttpEffect {
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        if matches!(
            d.output,
            Out::Effect {
                kind: EffectKind::OpenPr,
                ..
            }
        ) {
            self.calls.set(self.calls.get() + 1);
            if !self.brief || self.calls.get() == 1 {
                return Err(tl3_github::Error::Http {
                    status: self.status,
                }
                .into());
            }
        }
        self.fake.execute(d)
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.fake.reconcile(d)
    }
    fn look(&mut self, d: &Delivery) -> tl3_controller::Result<Look> {
        Ok(match self.reconcile(d)? {
            Reconciliation::Found(c) => Look::Found(c),
            Reconciliation::Absent => Look::Absent,
            Reconciliation::Pending => Look::Pending,
        })
    }
}

fn effect_http_failure(settings: &tl3_config::Store, case: &str) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("http-github")).unwrap();
    let clock = FakeClock::new();
    let calls = Rc::new(Cell::new(0));
    let mut c = open(
        settings,
        root.path(),
        &clock,
        Upstream::GitHub,
        HttpEffect {
            fake: adapters::github::Fake::new(&root.path().join("http-github")),
            calls: calls.clone(),
            status: if case == "forbidden" { 403 } else { 404 },
            brief: case == "brief",
        },
    );
    assert_eq!(calls.get(), 1);
    if case != "forbidden" {
        let first = c
            .snapshot()
            .unwrap()
            .pending_effects
            .into_iter()
            .find(|e| e.payload["output"]["Effect"]["kind"] == "OpenPr")
            .expect("the first effect 404 must stay pending for a look");
        assert_eq!(first.state, EffectState::Uncertain);
        assert_eq!(c.world().unwrap().nodes[0].fx_failures, 0);
        assert!(
            !c.snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "incidents")
        );
        if case == "webhooks" {
            for index in 0..3 {
                let mut issue = labelled();
                issue.delivery = format!("duplicate-webhook-{index}");
                c.intake(issue).unwrap();
                tl3_controller::ingress::apply(&mut c, tl3_controller::cli::Input::GitHubWake)
                    .unwrap();
                drain(&mut c);
                assert_eq!(
                    calls.get(),
                    1,
                    "webhooks and GitHub wakes must not release a held 404 before its look"
                );
            }
        }
        c.look_key(&first.key).unwrap();
        assert_eq!(c.world().unwrap().nodes[0].fx_failures, 1);
        drain(&mut c);
        assert_eq!(calls.get(), 2, "404 gets exactly one retry");
    }
    if case == "brief" {
        assert_eq!(
            c.world().unwrap().nodes[0].phase,
            Phase::Closed(tl3_kernel::Outcome::Merged)
        );
        assert!(
            !c.snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "incidents")
        );
    } else {
        let snapshot = c.snapshot().unwrap();
        let allowance = snapshot
            .rows
            .iter()
            .find(|r| {
                r.table == "allowances"
                    && r.value["delivery"]["output"]["Effect"]["kind"] == "OpenPr"
                    && r.value["failure"]
                        == if case == "forbidden" {
                            "permission"
                        } else {
                            "not_found"
                        }
            })
            .expect("the final HTTP failure must retain its permanent class");
        let key = allowance.value["delivery"]["key"].as_str().unwrap();
        assert_eq!(c.effect(key).unwrap().unwrap().state, EffectState::Failed);
    }
    let before = calls.get();
    for _ in 0..5 {
        assert!(!c.drive_once().unwrap(), "terminal failure must not loop");
    }
    assert_eq!(calls.get(), before);
    println!("test effect_http_{case} ... ok");
}
