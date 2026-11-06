//! Intentional red for state-table.md:784, owned by the closed-node recovery workstream.
#![allow(
    dead_code,
    reason = "Retained recording fixture shares enum cases with delivery conformance"
)]
#[path = "support/controller_restart.rs"]
mod controller_restart;
#[path = "../../store/tests/support/mod.rs"]
mod database;
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tl3_controller::{
    Bootstrap, Boundary, Controller, Repository,
    executor::{Completion, Delivery, Executor, Reconciliation, Upstream},
};
use tl3_store::{EffectState, EffectState as State, Store};
#[derive(Clone, Copy)]
enum Answer {
    Timeout,
    Found,
    Absent,
    Pending,
    Retryable,
    Ambiguous,
}
#[derive(Default)]
struct Script {
    answers: VecDeque<Answer>,
    target: Option<String>,
    late_resource: bool,
    observations: VecDeque<Vec<tl3_controller::executor::Observation>>,
    calls: Vec<String>,
}
struct Recording(Arc<Mutex<Script>>, tl3_config::Store);
impl Executor for Recording {
    fn observations(
        &mut self,
        delivery: &Delivery,
    ) -> tl3_controller::Result<Vec<tl3_controller::executor::Observation>> {
        let mut script = self.0.lock().unwrap();
        script.calls.push(format!("observe:{}", delivery.key));
        Ok(script.observations.pop_front().unwrap_or_default())
    }
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        assert_eq!(
            actual(&self.1, &delivery.key),
            State::Sent,
            "Sent must be durable before execute"
        );
        let mut script = self.0.lock().unwrap();
        if script
            .target
            .as_ref()
            .is_some_and(|key| *key != delivery.key)
        {
            return Ok(Completion::confirmed(vec![]));
        }
        script.calls.push(format!("execute:{}", delivery.key));
        if script.late_resource
            && matches!(
                delivery.output,
                tl3_controller::executor::Output::Effect {
                    kind: tl3_kernel::EffectKind::ClosePr | tl3_kernel::EffectKind::RemoveWorktree,
                    ..
                }
            )
        {
            script.late_resource = false;
            return Ok(Completion::confirmed(vec![]));
        }
        match script.answers.pop_front().unwrap() {
            Answer::Found => Ok(Completion::confirmed(vec![])),
            Answer::Retryable => Ok(Completion {
                ack: tl3_kernel::Ack::Retryable,
                ..Completion::confirmed(vec![])
            }),
            Answer::Ambiguous => Err(tl3_controller::Error::ApprovalAmbiguous),
            Answer::Timeout => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "scheduled lost response",
            )
            .into()),
            _ => panic!("execute response"),
        }
    }
    fn look(
        &mut self,
        delivery: &Delivery,
    ) -> tl3_controller::Result<tl3_controller::executor::Look> {
        self.0
            .lock()
            .unwrap()
            .calls
            .push(format!("look:{}", delivery.key));
        Ok(tl3_controller::executor::Look::Pending)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        let mut script = self.0.lock().unwrap();
        if script
            .target
            .as_ref()
            .is_some_and(|key| *key != delivery.key)
        {
            return Ok(Reconciliation::Found(Box::new(Completion::confirmed(
                vec![],
            ))));
        }
        script.calls.push(format!("reconcile:{}", delivery.key));
        Ok(match script.answers.pop_front().unwrap() {
            Answer::Found => Reconciliation::Found(Box::new(Completion::confirmed(vec![]))),
            Answer::Absent => Reconciliation::Absent,
            Answer::Pending => Reconciliation::Pending,
            _ => panic!("reconciliation response"),
        })
    }
}
fn repo() -> Repository {
    Repository {
        owner: "owner".into(),
        name: "sandbox".into(),
        installation: 3,
    }
}
fn actual(settings: &tl3_config::Store, key: &str) -> State {
    match Store::open(settings)
        .unwrap()
        .effect(key)
        .unwrap()
        .unwrap()
        .state
    {
        EffectState::Intended => State::Intended,
        EffectState::Sent => State::Sent,
        EffectState::Uncertain => State::Uncertain,
        EffectState::Confirmed => State::Confirmed,
        EffectState::Failed => State::Failed,
        EffectState::Cancelling => State::Cancelling,
        EffectState::Cancelled => State::Cancelled,
    }
}
fn drive(controller: &mut Controller, settings: &tl3_config::Store, key: &str, expected: State) {
    let mut boundaries = Vec::new();
    controller
        .drive_once_with(|boundary, own| boundaries.push((boundary, own.to_string())))
        .unwrap();
    assert_eq!(actual(settings, key), expected);
    if boundaries
        .iter()
        .any(|(b, _)| *b == Boundary::BeforeDispatch)
    {
        assert!(
            boundaries
                .iter()
                .any(|(b, _)| *b == Boundary::AfterDispatch)
        );
    }
}

fn ready_effect(settings: &tl3_config::Store, kind: tl3_kernel::EffectKind) -> String {
    let root = tempfile::tempdir().unwrap();
    let registry = tl3_controller::adapters::simulation(root.path()).unwrap();
    let mut controller = Controller::open(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap {
            repos: vec![repo()],
            rob: 42,
            slots: 1,
            pool: 1,
            fail_fast: false,
            operations: None,
        },
    )
    .unwrap();
    controller
        .intake(tl3_controller::LabelledIssue {
            delivery: "delivery-conformance".into(),
            owner: "owner".into(),
            repo: "sandbox".into(),
            installation: 3,
            number: 12,
            title: "Fix the value".into(),
            actor: 42,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    for _ in 0..300 {
        if let Some(effect) = controller
            .snapshot()
            .unwrap()
            .pending_effects
            .into_iter()
            .find(|e| {
                let d: Delivery = serde_json::from_value(e.payload.clone()).unwrap();
                matches!(d.output, tl3_controller::executor::Output::Effect { kind: actual, .. } if actual == kind)
            })
        {
            return effect.key;
        }
        assert!(controller.drive_once().unwrap());
    }
    panic!("no {kind:?}");
}
fn fixture_connection(settings: &tl3_config::Store) -> postgres::Client {
    postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap()
}

/// schedule an actual closed-node resource check independently of that final key.
#[test]
fn closed_node_restart_schedules_late_resource_recovery() {
    let mut database = database::Database::start();
    let settings = database.create("closed_late_recovery");
    let old_key = ready_effect(&settings, tl3_kernel::EffectKind::OpenPr);
    let root = tempfile::tempdir().unwrap();
    let mut sql = fixture_connection(&settings);
    controller_restart::wait_for_owner_release(&mut sql);
    let bootstrap = Bootstrap {
        repos: vec![repo()],
        rob: 42,
        slots: 1,
        pool: 1,
        fail_fast: false,
        operations: None,
    };
    let flight = Arc::new(Mutex::new(Script {
        target: Some(old_key.clone()),
        answers: VecDeque::from([Answer::Timeout, Answer::Absent]),
        ..Script::default()
    }));
    let mut registry = tl3_controller::adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::GitHub,
        Recording(flight.clone(), settings.clone()),
    );
    let mut controller =
        Controller::open(Store::open(&settings).unwrap(), registry, bootstrap.clone()).unwrap();
    // The real dispatch commits Sent before the request loses its response.
    drive(&mut controller, &settings, &old_key, State::Sent);
    assert!(
        flight
            .lock()
            .unwrap()
            .calls
            .contains(&format!("execute:{old_key}"))
    );
    drop(controller);
    controller_restart::wait_for_owner_release(&mut sql);
    let mut registry = tl3_controller::adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::GitHub,
        Recording(flight.clone(), settings.clone()),
    );
    let mut controller =
        Controller::open(Store::open(&settings).unwrap(), registry, bootstrap.clone()).unwrap();
    controller
        .observe(
            "stop-before-late-landing",
            tl3_controller::executor::Observation::Steering {
                node: tl3_kernel::NodeId(0),
                command: tl3_kernel::Command::Stop,
            },
        )
        .unwrap();
    assert!((0..200).any(|_| !controller.drive_once().unwrap()));
    let snapshot = controller.snapshot().unwrap();
    let node = snapshot
        .rows
        .iter()
        .find(|r| r.table == "nodes" && r.id == "0")
        .unwrap();
    assert!(
        node.value["phase"] == json!({"Closed":"Stopped"}),
        "teardown must finish before the zombie lands: {}",
        node.value["phase"]
    );
    let effect = Store::open(&settings)
        .unwrap()
        .effects()
        .unwrap()
        .into_iter()
        .find(|e| {
            let d: Delivery = serde_json::from_value(e.payload.clone()).unwrap();
            matches!(
                d.output,
                tl3_controller::executor::Output::Effect {
                    kind: tl3_kernel::EffectKind::OpenPr,
                    ..
                }
            )
        })
        .unwrap();
    assert!(!effect.state.is_pending());
    assert_eq!(effect.key, old_key);
    assert_eq!(effect.state, EffectState::Cancelled);
    assert!(flight.lock().unwrap().answers.is_empty());
    drop(controller);
    controller_restart::wait_for_owner_release(&mut sql);
    // No mutation is issued unless the real Controller invokes the executor.
    // The recorded upstream now contains the old operation's late PR/branch.
    let calls = Arc::new(Mutex::new(Script {
        late_resource: true,
        ..Script::default()
    }));
    let mut registry = tl3_controller::adapters::simulation(root.path()).unwrap();
    registry.register(Upstream::GitHub, Recording(calls.clone(), settings.clone()));
    registry.register(
        Upstream::Worktrees,
        Recording(calls.clone(), settings.clone()),
    );
    let mut restarted =
        Controller::open(Store::open(&settings).unwrap(), registry, bootstrap).unwrap();
    restarted.look_now().unwrap();
    assert!((0..30).any(|_| !restarted.drive_once().unwrap()));
    let calls = calls.lock().unwrap();
    assert!(
        !calls.late_resource,
        "late PR/branch survived restart and the real look/drive loop: {:?}",
        calls.calls
    );
    assert!(
        calls.calls.iter().any(|c| c.starts_with("look:")
            || c.starts_with("execute:")
            || c.starts_with("reconcile:")),
        "restart never checked or repaired closed-node resources after finality: {:?}",
        calls.calls
    );
}
