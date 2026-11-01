mod support;

use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex},
};
use support::{executor_probe::delivery, workers::Workers};
use tl3_controller::{
    Result,
    executor::{Completion, Delivery, Executor, Look, Reconciliation, WaitRecord},
};
use tl3_kernel::{EffectKind, Role};
use tl3_store::{ExternalWait, Store};

type Calls = Arc<Mutex<Vec<(&'static str, &'static str, String)>>>;
struct Spy {
    name: &'static str,
    calls: Calls,
}
impl Spy {
    fn hit<T>(&self, method: &'static str, argument: String) -> Result<T> {
        self.calls
            .lock()
            .unwrap()
            .push((self.name, method, argument));
        Err(tl3_controller::Error::Invalid(self.name))
    }
}
impl Executor for Spy {
    fn input_incident(
        &mut self,
        notice: &tl3_controller::input_errors::IncidentNotice,
        epoch: i64,
    ) -> Result<()> {
        self.hit("input_incident", format!("{}:{epoch}", notice.key))
    }
    fn intake_question(
        &mut self,
        question: &tl3_controller::adoption::Clarification,
        epoch: i64,
    ) -> Result<()> {
        self.hit("intake_question", format!("{}:{epoch}", question.key()))
    }
    fn app_login(&self, installation: u64) -> Option<String> {
        let _: Result<()> = self.hit("app_login", installation.to_string());
        Some(self.name.into())
    }
    fn implementation(&self) -> &'static str {
        let _: Result<()> = self.hit("implementation", String::new());
        self.name
    }
    fn watch_wait(&mut self, delivery: &Delivery, _: Option<&ExternalWait>) -> Result<()> {
        self.hit("watch_wait", delivery.key.clone())
    }
    fn wait_record(&mut self, key: &str, checkout: &Path) -> Result<WaitRecord> {
        self.hit("wait_record", format!("{key}:{}", checkout.display()))
    }
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.hit("execute", delivery.key.clone())
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.hit("reconcile", delivery.key.clone())
    }
    fn reconcile_cancellation(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.hit("reconcile_cancellation", delivery.key.clone())
    }
    fn look(&mut self, delivery: &Delivery) -> Result<Look> {
        self.hit("look", delivery.key.clone())
    }
    fn probe(&mut self, delivery: &Delivery) -> Result<bool> {
        self.hit("probe", delivery.key.clone())
    }
    fn resume_after_quota(&mut self, delivery: &Delivery) -> Result<Option<Completion>> {
        self.hit("resume_after_quota", delivery.key.clone())
    }
    fn stop_stalled(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.hit("stop_stalled", delivery.key.clone())
    }
    fn recover_deliveries(&mut self, after: u64, since_ms: u64) -> Result<Option<u64>> {
        self.hit("recover_deliveries", format!("{after}:{since_ms}"))
    }
    fn retire_delivery(&mut self, delivery: &Delivery) -> Result<()> {
        self.hit("retire_delivery", delivery.key.clone())
    }
    fn retire(&mut self, _: &mut Store) -> Result<()> {
        self.hit("retire", String::new())
    }
}

fn methods<'a>(source: &'a str, start: &str) -> BTreeSet<&'a str> {
    let body = source
        .split_once(start)
        .unwrap()
        .1
        .split_once("\n}\n")
        .unwrap()
        .0;
    body.lines()
        .filter_map(|line| line.trim().strip_prefix("fn "))
        .map(|signature| signature.split_once('(').unwrap().0)
        .collect()
}

#[test]
fn shared_router_and_probe_cover_the_executor_trait() {
    let expected = methods(include_str!("../src/executor.rs"), "pub trait Executor {");
    assert!(!expected.is_empty());
    assert_eq!(
        methods(
            include_str!("support/workers.rs"),
            "Executor for Workers<F> {"
        ),
        expected,
        "every Executor method must be forwarded, including new default methods"
    );
    assert_eq!(
        methods(
            include_str!("fixture_contracts.rs"),
            "impl Executor for Spy {"
        ),
        expected,
        "every Executor method must have a distinguishable forwarding probe"
    );
}

#[test]
fn workers_forward_to_the_selected_adapter_for_every_role() {
    let mut database = support::Database::start();
    let mut store = Store::open(&database.create("router_contract")).unwrap();
    for real_roles in [
        vec![],
        vec![Role::Make],
        vec![Role::Make, Role::Review],
        vec![Role::Review],
    ] {
        let calls = Calls::default();
        let real: Option<Box<dyn Executor>> = (!real_roles.is_empty()).then(|| {
            Box::new(Spy {
                name: "real",
                calls: calls.clone(),
            }) as Box<dyn Executor>
        });
        let primary = if real.is_some() { "real" } else { "fake" };
        let mut router = Workers::new(
            real,
            Spy {
                name: "fake",
                calls: calls.clone(),
            },
            real_roles.clone(),
            |_, _| panic!("an adapter error must propagate"),
        );
        for role in [
            Role::Ingest,
            Role::Audit,
            Role::Plan,
            Role::Make,
            Role::Review,
        ] {
            let d = delivery(role);
            let selected = if real_roles.contains(&role) {
                "real"
            } else {
                "fake"
            };
            assert_delivery_methods(&mut router, &d, selected, &calls);
        }
        let mut stop = delivery(Role::Make);
        stop.output = tl3_controller::executor::Output::Effect {
            node: stop.node(),
            seq: 2,
            kind: EffectKind::StopRun,
            head: None,
        };
        assert_delivery_methods(&mut router, &stop, primary, &calls);
        assert_eq!(router.implementation(), primary);
        assert_eq!(router.app_login(42).as_deref(), Some(primary));
        let question = tl3_controller::adoption::Clarification {
            repository: delivery(Role::Make).repository,
            number: 17,
        };
        assert!(router.intake_question(&question, 3).is_err());
        let incident = tl3_controller::input_errors::IncidentNotice {
            key: "incident:probe".into(),
            target: tl3_controller::input_errors::IssueTarget {
                repository: question.repository.clone(),
                issue: 17,
            },
            title: "A request".into(),
            summary: "Work needs help.".into(),
            id: "probe".into(),
        };
        assert!(router.input_incident(&incident, 3).is_err());
        assert!(router.wait_record("key", Path::new("checkout")).is_err());
        assert!(router.recover_deliveries(17, 29).is_err());
        assert!(router.retire(&mut store).is_err());
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                (primary, "implementation", String::new()),
                (primary, "app_login", "42".into()),
                (primary, "intake_question", format!("{}:3", question.key())),
                (primary, "input_incident", "incident:probe:3".into()),
                (primary, "wait_record", "key:checkout".into()),
                (primary, "recover_deliveries", "17:29".into()),
                (primary, "retire", String::new()),
            ]
        );
    }
    drop(store);
    database.stop();
}

fn assert_delivery_methods(
    router: &mut dyn Executor,
    d: &Delivery,
    selected: &'static str,
    calls: &Calls,
) {
    calls.lock().unwrap().clear();
    assert!(router.execute(d).is_err());
    assert!(router.reconcile(d).is_err());
    assert!(router.reconcile_cancellation(d).is_err());
    assert!(router.look(d).is_err());
    assert!(router.probe(d).is_err());
    assert!(router.resume_after_quota(d).is_err());
    assert!(router.stop_stalled(d).is_err());
    assert!(router.watch_wait(d, None).is_err());
    assert!(router.retire_delivery(d).is_err());
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "execute",
            "reconcile",
            "reconcile_cancellation",
            "look",
            "probe",
            "resume_after_quota",
            "stop_stalled",
            "watch_wait",
            "retire_delivery"
        ]
        .map(|method| (selected, method, d.key.clone()))
    );
    calls.lock().unwrap().clear();
}
