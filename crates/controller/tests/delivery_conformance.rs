//! CC-E: the mirror is checked against real Controller / Store steps, without
//! modifying the read-only drive_once or dispatch-liveness implementation.
#[path = "../../store/tests/support/mod.rs"]
mod database;
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tl3_controller::{
    Bootstrap, Boundary, Controller, Repository,
    executor::{Completion, Delivery, Executor, Reconciliation, Registry, Upstream},
};
use tl3_delivery::{
    Ack, State,
    mirror::{self, Action, Read},
};
use tl3_store::{Commit, EffectState, EffectTransition, NewEffect, Store};

#[derive(Clone, Copy)]
enum Answer {
    Timeout,
    Found,
    Absent,
    Pending,
}
#[derive(Default)]
struct Script {
    answers: VecDeque<Answer>,
    calls: Vec<String>,
}
struct Recording(Arc<Mutex<Script>>);
impl Executor for Recording {
    fn observations(
        &mut self,
        delivery: &Delivery,
    ) -> tl3_controller::Result<Vec<tl3_controller::executor::Observation>> {
        self.0
            .lock()
            .unwrap()
            .calls
            .push(format!("observe:{}", delivery.key));
        Ok(vec![])
    }
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        let mut script = self.0.lock().unwrap();
        script.calls.push(format!("execute:{}", delivery.key));
        match script.answers.pop_front().unwrap() {
            Answer::Found => Ok(Completion::confirmed(vec![])),
            Answer::Timeout => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "scheduled lost response",
            )
            .into()),
            _ => panic!("execute response"),
        }
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        let mut script = self.0.lock().unwrap();
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
    registry.register(Upstream::GitHub, Recording(script));
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

/// State-table Effects / F2 / ack-by-key: real separate Sent, timeout, Uncertain,
/// held Absent, Pending, late Found, and a second untouched original key.
#[test]
fn controller_conforms_to_timeout_hold_and_late_ack_steps() {
    let mut database = database::Database::start();
    let settings = database.create("delivery_mirror");
    let script = Arc::new(Mutex::new(Script {
        answers: VecDeque::from([
            Answer::Timeout,
            Answer::Absent,
            Answer::Pending,
            Answer::Found,
        ]),
        ..Script::default()
    }));
    let mut controller = open(&settings, script.clone());
    let epoch = controller.snapshot().unwrap().epoch.get();
    seed(&settings, "a-original", epoch);
    let mut state = State::Intended;
    state = mirror::committed(state, mirror::dispatch(state, false, false), Ack::Confirmed);
    drive(&mut controller, &settings, "a-original", state);
    state = mirror::committed(state, mirror::dispatch(state, false, false), Ack::Confirmed);
    drive(&mut controller, &settings, "a-original", state);
    for read in [Read::Absent, Read::Pending] {
        state = mirror::committed(
            state,
            mirror::reconciliation(state, read, true, false, false),
            Ack::Confirmed,
        );
        drive(&mut controller, &settings, "a-original", state);
    }
    seed(&settings, "z-other", epoch);
    state = mirror::committed(
        state,
        mirror::reconciliation(state, Read::Found, true, false, false),
        Ack::Confirmed,
    );
    drive(&mut controller, &settings, "a-original", state);
    assert_eq!(actual(&settings, "z-other"), State::Intended);
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

/// State-table old-epoch cancellation is mirrored against the real Controller.
#[test]
fn old_intended_epoch_cancels_without_http() {
    let mut database = database::Database::start();
    let settings = database.create("delivery_old");
    let script = Arc::new(Mutex::new(Script::default()));
    let mut controller = open(&settings, script.clone());
    let epoch = controller.snapshot().unwrap().epoch.get();
    seed(&settings, "old", epoch);
    let mut store = Store::open(&settings).unwrap();
    let snapshot = store.load().unwrap();
    let mut advance = Commit::new("new-epoch", json!(null), &snapshot);
    advance.next_epoch = Some(tl3_store::Epoch::try_from(epoch + 1).unwrap());
    store.commit(&advance).unwrap();
    let action = mirror::dispatch(State::Intended, true, false);
    assert_eq!(action, Action::Cancel);
    drive(
        &mut controller,
        &settings,
        "old",
        mirror::committed(State::Intended, action, Ack::Retryable),
    );
    assert!(script.lock().unwrap().calls.is_empty());
}
