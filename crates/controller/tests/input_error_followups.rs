#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::json;
use std::{
    cell::Cell,
    path::Path,
    rc::Rc,
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tl3_controller::{
    Bootstrap, Controller, Error, LabelledIssue, Repository, adapters,
    cli::Input,
    clock::Clock,
    executor::{Completion, Delivery, Executor, Look, Observation, Reconciliation, Upstream},
};
use tl3_kernel::{Event, NodeId};
use tl3_store::Store;

#[derive(Clone)]
struct TestClock(Rc<Cell<u64>>);
impl Clock for TestClock {
    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(self.0.get())
    }
}
fn boot() -> Bootstrap {
    Bootstrap {
        repos: vec![Repository {
            owner: "owner".into(),
            name: "sandbox".into(),
            installation: 3,
        }],
        rob: 42,
        slots: 1,
        pool: 1,
        operations: None,
    }
}
fn issue() -> LabelledIssue {
    LabelledIssue {
        delivery: "label".into(),
        owner: "owner".into(),
        repo: "sandbox".into(),
        installation: 3,
        number: 12,
        title: "Fix greeting".into(),
        actor: 42,
        label: "tl3".into(),
    }
}
fn sql(settings: &tl3_config::Store, statement: &str) {
    postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap()
        .batch_execute(statement)
        .unwrap();
}
fn drain(c: &mut Controller) {
    for _ in 0..200 {
        if !c.drive_once().unwrap() {
            return;
        }
    }
    panic!("controller did not settle");
}
fn run(c: &mut Controller, input: Input) -> tl3_controller::Result<()> {
    let (tx, rx) = mpsc::channel();
    tx.send(input).unwrap();
    drop(tx);
    tl3_controller::ingress::run(c, rx)
}
fn open(settings: &tl3_config::Store, root: &Path) -> Controller {
    Controller::open(
        Store::open(settings).unwrap(),
        adapters::simulation(root).unwrap(),
        boot(),
    )
    .unwrap()
}
fn reject_interns(settings: &tl3_config::Store) {
    sql(
        settings,
        "CREATE FUNCTION reject_intern() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected store failure'; END $$; CREATE TRIGGER reject_intern BEFORE INSERT ON tl3_intern FOR EACH ROW EXECUTE FUNCTION reject_intern();",
    );
}
#[test]
fn intake_store_failure_stops_ingress() {
    let mut db = support::Database::start();
    let settings = db.create("intake_store");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    reject_interns(&settings);
    assert!(
        matches!(
            run(&mut c, Input::LabelledIssue(issue())),
            Err(Error::Store(_))
        ),
        "intake store failure must stop ingress"
    );
}
#[test]
fn observation_store_failure_stops_ingress() {
    let mut db = support::Database::start();
    let settings = db.create("observation_store");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    reject_interns(&settings);
    assert!(
        matches!(
            run(
                &mut c,
                Input::Observation {
                    key: "made".into(),
                    observation: Observation::Made {
                        node: NodeId(0),
                        generation: 1,
                        sha: "a".repeat(40)
                    }
                }
            ),
            Err(Error::Store(_))
        ),
        "observation store failure must stop ingress"
    );
}
struct BrokenLook {
    fake: adapters::workers::Fake,
    settings: tl3_config::Store,
    corruption: bool,
    calls: Rc<Cell<usize>>,
}
impl Executor for BrokenLook {
    fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
        Err(std::io::Error::from(std::io::ErrorKind::ConnectionReset).into())
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(Reconciliation::Pending)
    }
    fn look(&mut self, d: &Delivery) -> tl3_controller::Result<Look> {
        self.calls.set(self.calls.get() + 1);
        if !self.corruption {
            return Err(Error::Store(tl3_store::Error::Invalid(
                "injected store failure",
            )));
        }
        // Corrupt the disposable DB after the outer look has hydrated its snapshot.
        sql(
            &self.settings,
            &format!(
                "ALTER TABLE tl3_effects DISABLE TRIGGER tl3_effect_guard; UPDATE tl3_effects SET payload = '{{}}' WHERE effect_key = '{}'; ALTER TABLE tl3_effects ENABLE TRIGGER tl3_effect_guard;",
                d.key
            ),
        );
        Ok(Look::Found(Completion::confirmed(vec![])))
    }
    fn retire_delivery(&mut self, d: &Delivery) -> tl3_controller::Result<()> {
        self.fake.retire_delivery(d)
    }
}
fn broken_look(corruption: bool) {
    let mut db = support::Database::start();
    let settings = db.create("broken_look");
    let root = tempfile::tempdir().unwrap();
    let calls = Rc::new(Cell::new(0));
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Workers,
        BrokenLook {
            fake: adapters::workers::Fake::new(root.path()),
            settings: settings.clone(),
            corruption,
            calls: calls.clone(),
        },
    );
    let mut c = Controller::open(Store::open(&settings).unwrap(), registry, boot()).unwrap();
    c.intake(issue()).unwrap();
    drain(&mut c);
    let result = run(&mut c, Input::Wake);
    assert!(
        calls.get() > 0,
        "injection must run inside the look boundary"
    );
    if corruption {
        assert!(
            result.is_err(),
            "corrupt stored effect payload must stop ingress instead of opening a setup incident"
        );
        assert!(
            !c.snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "incidents"),
            "stored corruption must not become an incident"
        );
    } else {
        assert!(
            matches!(result, Err(Error::Store(_))),
            "look store failure must stop ingress"
        );
    }
}
#[test]
fn corrupt_completion_payload_stops_ingress() {
    broken_look(true);
}
#[test]
fn look_store_failure_stops_ingress() {
    broken_look(false);
}

#[test]
fn corrupt_stored_inbox_stops_ingress() {
    let mut db = support::Database::start();
    let settings = db.create("corrupt_inbox");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    sql(
        &settings,
        "INSERT INTO tl3_world (table_name, row_id, value) VALUES ('inbox', 'broken', '{}');",
    );
    assert!(
        run(&mut c, Input::Wake).is_err(),
        "corrupt stored inbox must stop ingress"
    );
}

struct RefusedPosts(Rc<Cell<usize>>);
impl Executor for RefusedPosts {
    fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
        unreachable!()
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        unreachable!()
    }
    fn input_incident(
        &mut self,
        _: &tl3_controller::input_errors::IncidentNotice,
        _: i64,
    ) -> tl3_controller::Result<()> {
        self.0.set(self.0.get() + 1);
        Err(tl3_github::Error::Http { status: 403 }.into())
    }
}
#[test]
fn failed_post_backoff_survives_wakes_and_restart() {
    let mut db = support::Database::start();
    let settings = db.create("post_backoff");
    let root = tempfile::tempdir().unwrap();
    let clock = TestClock(Rc::new(Cell::new(1000)));
    let calls = Rc::new(Cell::new(0));
    let reopen = || {
        let mut registry = adapters::simulation(root.path()).unwrap();
        registry.register(Upstream::GitHub, RefusedPosts(calls.clone()));
        let mut bootstrap = boot();
        bootstrap.operations = Some(tl3_config::OperationsIssue {
            owner: "owner".into(),
            name: "operations".into(),
            installation_id: 3,
            number: 17,
        });
        Controller::open_with_clock(
            Store::open(&settings).unwrap(),
            registry,
            bootstrap,
            clock.clone(),
        )
        .unwrap()
    };
    let mut c = reopen();
    c.observe(
        "invalid",
        Observation::Kernel(Event::NewRequest {
            repo: tl3_kernel::RepoId(0),
        }),
    )
    .unwrap();
    drain(&mut c);
    assert_eq!(calls.get(), 1);
    for _ in 0..3 {
        tl3_controller::ingress::apply(&mut c, Input::GitHubWake).unwrap();
        drain(&mut c);
    }
    assert_eq!(
        calls.get(),
        1,
        "permanently failing posts must back off across GitHub wakes"
    );
    drop(c);
    let mut c = reopen();
    c.look_now().unwrap();
    drain(&mut c);
    assert_eq!(
        calls.get(),
        1,
        "post backoff must survive controller restart"
    );
    clock.0.set(clock.0.get() + 3600);
    tl3_controller::ingress::apply(&mut c, Input::GitHubWake).unwrap();
    drain(&mut c);
    assert_eq!(calls.get(), 2, "a later wake must retry after backoff");
}

#[test]
fn missing_destination_logs() {
    if std::env::var_os("TL3_MISSING_DESTINATION_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "missing_destination_logs", "--nocapture"])
            .env("TL3_MISSING_DESTINATION_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("no issue or operations destination"),
            "unpostable incident needs an explicit destination diagnostic: {stderr}"
        );
        return;
    }
    let mut db = support::Database::start();
    let settings = db.create("missing_destination");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    c.observe(
        "invalid",
        Observation::Kernel(Event::NewRequest {
            repo: tl3_kernel::RepoId(0),
        }),
    )
    .unwrap();
    drain(&mut c);
    assert_eq!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.table == "incidents")
            .count(),
        1
    );
}
