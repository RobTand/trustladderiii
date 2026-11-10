//! Real Controller and Store delivery boundaries, epoch carry, and approval retries.
use crate::database;
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tl3_controller::{
    Bootstrap, Boundary, Controller, Repository,
    executor::{Completion, Delivery, Executor, Reconciliation, Registry, Upstream},
};
use tl3_delivery::State;
use tl3_store::{Commit, EffectState, EffectTransition, NewEffect, Store};

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
fn open(settings: &tl3_config::Store, script: Arc<Mutex<Script>>) -> Controller {
    let mut registry = Registry::default();
    registry.register(Upstream::GitHub, Recording(script, settings.clone()));
    Controller::open(
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
    .unwrap()
}
fn seed(settings: &tl3_config::Store, key: &str, epoch: i64) {
    let mut store = Store::open(settings).unwrap();
    let snapshot = store.load().unwrap();
    let payload = json!({"key":key,"epoch":epoch,
        "output":{"Comment":{"node":0,"kind":"Clarify"}},"generation":0,"head_sha":null,
        "repository":repo(),"issue":12,"pull_request":null,"paths":[],"attempt":0,"round":0});
    let mut change = Commit::new(format!("seed:{key}"), json!(null), &snapshot);
    change.effects.push(NewEffect {
        key: key.into(),
        kind: "GitHub".into(),
        payload,
    });
    store.commit(&change).unwrap();
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

/// State-table cancellation rows and F2: exercise the real ActiveDeliveries SQL
/// guard in every active state, and the successful drain before epoch advancement.
#[test]
fn real_store_refuses_epoch_advance_until_deliveries_drain() {
    let mut database = database::Database::start();
    let settings = database.create("delivery_drain");
    let script = Arc::new(Mutex::new(Script::default()));
    let mut controller = open(&settings, script);
    let epoch = controller.snapshot().unwrap().epoch.get();
    seed(&settings, "drain", epoch);
    let mut store = Store::open(&settings).unwrap();
    let mut from = EffectState::Intended;
    for (index, to) in [
        EffectState::Sent,
        EffectState::Uncertain,
        EffectState::Cancelling,
        EffectState::Cancelled,
    ]
    .into_iter()
    .enumerate()
    {
        let snapshot = store.load().unwrap();
        let mut change = Commit::new(format!("state:{index}"), json!(null), &snapshot);
        change.transitions.push(EffectTransition {
            key: "drain".into(),
            from,
            to,
        });
        store.commit(&change).unwrap();
        let snapshot = store.load().unwrap();
        let mut advance = Commit::new(format!("advance:{index}"), json!(null), &snapshot);
        advance.next_epoch = Some(tl3_store::Epoch::try_from(epoch + 1).unwrap());
        if tl3_delivery::blocks_epoch_advance(actual(&settings, "drain")) {
            assert!(matches!(
                store.commit(&advance),
                Err(tl3_store::Error::ActiveDeliveries)
            ));
        } else {
            store.commit(&advance).unwrap();
        }
        from = to;
    }
}

/// Store lib.rs:1133-1140: a drained epoch carries Intended rows forward before dispatch.
#[test]
fn intended_is_carried_into_new_epoch_before_dispatch() {
    let mut database = database::Database::start();
    let settings = database.create("delivery_old");
    let script = Arc::new(Mutex::new(Script {
        answers: VecDeque::from([Answer::Found]),
        ..Script::default()
    }));
    let mut controller = open(&settings, script.clone());
    let epoch = controller.snapshot().unwrap().epoch.get();
    seed(&settings, "old", epoch);
    let mut store = Store::open(&settings).unwrap();
    let snapshot = store.load().unwrap();
    let mut advance = Commit::new("new-epoch", json!(null), &snapshot);
    advance.next_epoch = Some(tl3_store::Epoch::try_from(epoch + 1).unwrap());
    store.commit(&advance).unwrap();
    assert_eq!(store.effect("old").unwrap().unwrap().epoch.get(), epoch + 1);
    assert_eq!(actual(&settings, "old"), State::Intended);
    drive(&mut controller, &settings, "old", State::Confirmed);
    assert_eq!(
        script
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|s| s.starts_with("execute:"))
            .count(),
        1
    );
}

use crate::controller_restart;

/// Complete durable projection at each real committed boundary. The enqueue
/// commit and the input transaction are observed independently through Store's
/// hook; no expected intermediate state is inferred from the final state.
#[derive(Clone, Debug)]
struct Durable {
    key: String,
    event: serde_json::Value,
    effect: State,
    rows: Vec<(String, String, serde_json::Value)>,
}
type Trace = Arc<Mutex<Vec<Durable>>>;
fn traced_open(
    settings: &tl3_config::Store,
    key: &str,
    script: Arc<Mutex<Script>>,
) -> (Controller, Trace) {
    script.lock().unwrap().target = Some(key.into());
    let mut sql = fixture_connection(settings);
    controller_restart::wait_for_owner_release(&mut sql);
    let mut store = Store::open(settings).unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let trace = captured.clone();
    let settings_copy = settings.clone();
    let target = key.to_string();
    store.set_boundary_hook(move |point, key, event| {
        if matches!(
            point,
            tl3_store::SavePoint::AfterCommit | tl3_store::SavePoint::InputAfterCommit
        ) {
            let mut other = Store::open(&settings_copy).unwrap();
            let snapshot = other.load().unwrap();
            if other.effect(&target).unwrap().is_some() {
                let mut rows: Vec<_> = snapshot
                    .rows
                    .into_iter()
                    .map(|r| (r.table, r.id, r.value))
                    .collect();
                rows.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
                trace.lock().unwrap().push(Durable {
                    key: key.into(),
                    event: event.clone(),
                    effect: actual(&settings_copy, &target),
                    rows,
                });
            }
        }
    });
    let mut registry = Registry::default();
    registry.register(Upstream::GitHub, Recording(script, settings.clone()));
    let controller = Controller::open(
        store,
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
    captured.lock().unwrap().clear();
    (controller, captured)
}
fn ready_approval(settings: &tl3_config::Store) -> String {
    ready_effect(settings, tl3_kernel::EffectKind::PostApproval)
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
fn has_row(d: &Durable, table: &str, key: &str) -> bool {
    d.rows.iter().any(|r| r.0 == table && r.1 == key)
}

/// R2: unknown dispatch needs clear. Stop after the real input transaction has
/// created the grant and restart BEFORE Sent. The next Sent consumes it once.
#[test]
fn approval_clear_is_durable_and_single_use_across_restart() {
    let mut database = database::Database::start();
    for restart in [false, true] {
        let settings = database.create(if restart {
            "grant_restart"
        } else {
            "grant_live"
        });
        let key = ready_approval(&settings);
        let script = Arc::new(Mutex::new(Script {
            answers: VecDeque::from([Answer::Ambiguous, Answer::Pending]),
            ..Script::default()
        }));
        let (mut controller, _) = traced_open(&settings, &key, script.clone());
        controller.drive_once().unwrap();
        controller.drive_once().unwrap();
        controller.drive_once().unwrap();
        assert_eq!(
            script
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|s| s.starts_with("execute:"))
                .count(),
            1
        );
        let snapshot = controller.snapshot().unwrap();
        let incident = snapshot
            .rows
            .iter()
            .find(|r| r.table == "incidents" && r.value["open"] == true && r.value["effect"] == key)
            .unwrap();
        controller
            .observe(
                "one-clear",
                tl3_controller::executor::Observation::GitHubComment {
                    repository: repo(),
                    issue: 12,
                    comment: tl3_github::Comment {
                        id: 900,
                        body: format!("clear incident {}", incident.id),
                        user: tl3_github::Actor {
                            id: 42,
                            login: "rob".into(),
                        },
                        created_at: chrono::DateTime::UNIX_EPOCH,
                        updated_at: chrono::DateTime::UNIX_EPOCH,
                        issue_url: "https://api.github.com/repos/owner/sandbox/issues/12".into(),
                    },
                },
            )
            .unwrap();
        // Do not drain here: that would pass the grant/Sent crash cut.
        for _ in 0..20 {
            if controller
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "approval_retry_authorizations" && r.id == key)
            {
                break;
            }
            controller.drive_once().unwrap();
        }
        assert_eq!(actual(&settings, &key), State::Intended);
        assert!(
            controller
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "approval_retry_authorizations" && r.id == key)
        );
        if restart {
            drop(controller);
            (controller, _) = traced_open(&settings, &key, script.clone());
        }
        script.lock().unwrap().answers = VecDeque::from([Answer::Ambiguous, Answer::Pending]);
        let (mut controller, trace) = if restart {
            drop(controller);
            traced_open(&settings, &key, script.clone())
        } else {
            (controller, Arc::new(Mutex::new(Vec::new())))
        };
        controller.drive_once().unwrap();
        assert_eq!(
            script
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|s| s.starts_with("execute:"))
                .count(),
            2
        );
        assert!(
            !controller
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "approval_retry_authorizations" && r.id == key)
        );
        controller.drive_once().unwrap();
        controller.drive_once().unwrap();
        assert_eq!(
            script
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|s| s.starts_with("execute:"))
                .count(),
            2
        );
        if restart {
            let trace = trace.lock().unwrap();
            let sent = trace
                .iter()
                .find(|d| d.key.starts_with("delivery:") && d.event["to"] == "Sent")
                .unwrap();
            assert!(!has_row(sent, "approval_retry_authorizations", &key));
        }
    }
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

/// R1: Sent is durable even when a queued observation returns before execute.
/// Recovery consumes the observation input before considering the old key again.
#[test]
fn observations_return_before_http_with_durable_sent_and_inbox() {
    let mut database = database::Database::start();
    let settings = database.create("observation_cut");
    let script = Arc::new(Mutex::new(Script {
        observations: VecDeque::from([vec![
            tl3_controller::executor::Observation::ScratchPruned {
                reference: "late-reference".into(),
            },
        ]]),
        ..Script::default()
    }));
    let mut initial = open(&settings, script.clone());
    let epoch = initial.snapshot().unwrap().epoch.get();
    seed(&settings, "observed", epoch);
    drop(initial);
    let (mut controller, trace) = traced_open(&settings, "observed", script.clone());
    controller.drive_once().unwrap();
    assert_eq!(actual(&settings, "observed"), State::Sent);
    assert_eq!(
        script.lock().unwrap().calls,
        vec!["observe:observed".to_string()]
    );
    assert!(
        !script
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|s| s.starts_with("execute:"))
    );
    let snapshot = controller.snapshot().unwrap();
    assert!(snapshot.rows.iter().any(|r| r.table == "inbox"));
    let trace = trace.lock().unwrap();
    let sent = trace
        .iter()
        .position(|d| d.key.starts_with("delivery:"))
        .unwrap();
    let observed = trace
        .iter()
        .rposition(|d| d.rows.iter().any(|r| r.0 == "inbox"))
        .unwrap();
    assert!(sent < observed);
    assert_eq!(trace[observed].effect, State::Sent);
}
