#[path = "../../store/tests/support/mod.rs"]
mod support;
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
use tl3_kernel::{Command, EffectKind, Event, NodeId, Out, Phase};
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
    // The fake GitHub receipt proves the incident was sent to the operations target.
    let deliveries = adapters::delivered_notes(root.path()).unwrap();
    assert!(deliveries.iter().any(|text| text.contains(&id)));
    let status = status::load(&mut Reader::open(settings).unwrap()).unwrap();
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
    quota: bool,
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
            if self.quota {
                return Err(tl3_controller::Error::Quota {
                    retry_after: Duration::from_secs(5),
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
            quota: mode == 3,
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
    struct Permanent;
    impl Executor for Permanent {
        fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
            Ok(Completion {
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
    comment(&mut c, &id, 42, 17, 2);
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
fn main() {
    let mut db = support::Database::start();
    stalled_runs_and_incident_controls(&db.create("stalls"));
    println!("test stalled_runs_and_incident_controls ... ok");
    for (mode, name) in [
        "held_found",
        "held_cancelled",
        "write_outage_incident",
        "quota_reset",
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
    permanent_incident_clear(&db.create("permanent_incident"));
    println!("test permanent_incident_clear ... ok");
    db.stop();
    println!("8 allowance and incident scenarios passed; no skips");
}
