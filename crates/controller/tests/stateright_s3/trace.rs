use super::model::{Action, Contract, EffectModel, Phase, State};
use crate::support::Database;
use stateright::Model;
use std::{cell::Cell, path::Path, rc::Rc};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    executor::{Completion, Delivery, Executor, Reconciliation, Upstream},
};
use tl3_store::{EffectState, Store};

fn step(model: &EffectModel, state: &mut State, action: Action) {
    let mut enabled = Vec::new();
    model.actions(state, &mut enabled);
    assert!(
        enabled.contains(&action),
        "disabled trace action {action:?}: {state:?}"
    );
    *state = model.next_state(state, action).unwrap();
}

#[derive(Clone, Copy)]
enum Fault {
    Lost,
    Pending,
    Healthy,
}

struct LostWorker {
    fake: adapters::workers::Fake,
    fault: Rc<Cell<Fault>>,
}

impl Executor for LostWorker {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        let completion = self.fake.execute(delivery)?;
        if matches!(self.fault.get(), Fault::Lost) {
            return Err(tl3_controller::Error::Adapter(Box::new(
                std::io::Error::other("S3: upstream applied, response lost"),
            )));
        }
        Ok(completion)
    }

    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        if matches!(self.fault.get(), Fault::Pending) {
            Ok(Reconciliation::Pending)
        } else {
            self.fake.reconcile(delivery)
        }
    }
}

fn compare(controller: &mut Controller, settings: &tl3_config::Store, root: &Path, state: &State) {
    let real = Store::open(settings)
        .unwrap()
        .effect("effect:0:0")
        .unwrap()
        .unwrap();
    let effect = &state.effects[0];
    let expected = match effect.phase {
        Phase::Intended => EffectState::Intended,
        Phase::Sent | Phase::Delivering | Phase::Response | Phase::Unknown => EffectState::Sent,
        Phase::Uncertain | Phase::Found | Phase::Absent => EffectState::Uncertain,
        Phase::Confirmed => EffectState::Confirmed,
        Phase::Fenced => EffectState::Cancelled,
    };
    assert_eq!(real.state, expected, "real outbox differs from S3");
    assert_eq!(real.key, "effect:0:0");
    assert_eq!(real.epoch.get(), i64::from(effect.epoch));
    let deliveries = adapters::delivery_counts(root)
        .unwrap()
        .get(&real.key)
        .copied()
        .unwrap_or(0);
    assert_eq!(
        deliveries,
        u64::from(effect.mutations),
        "real upstream record differs from S3"
    );
    assert_eq!(effect.recorded, deliveries != 0);
    let pending = controller
        .snapshot()
        .unwrap()
        .pending_effects
        .iter()
        .any(|e| e.key == real.key);
    assert_eq!(pending, expected.is_pending());
}

#[test]
fn lost_response_keeps_the_key_real_trace_conforms() {
    let mut database = Database::start();
    let settings = database.create("s3_trace");
    let root = tempfile::tempdir().unwrap();
    let fault = Rc::new(Cell::new(Fault::Lost));
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Workers,
        LostWorker {
            fake: adapters::workers::Fake::new(root.path()),
            fault: fault.clone(),
        },
    );
    let mut controller = Controller::open(
        Store::open(&settings).unwrap(),
        registry,
        Bootstrap {
            operations: None,
            repos: vec![Repository {
                owner: "owner".into(),
                name: "sandbox".into(),
                installation: 1,
            }],
            rob: 42,
            slots: 1,
            pool: 1,
            fail_fast: false,
        },
    )
    .unwrap();
    controller
        .intake(LabelledIssue {
            delivery: "s3-lost-response".into(),
            owner: "owner".into(),
            repo: "sandbox".into(),
            installation: 1,
            number: 7,
            title: "A real S3 trace".into(),
            actor: 42,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    let model = EffectModel::new(vec![Contract::Once]);
    let mut state = model.init_states().remove(0);
    compare(&mut controller, &settings, root.path(), &state);

    assert!(controller.drive_once().unwrap());
    for action in [
        Action::CommitSent(0),
        Action::Dispatch(0),
        Action::Deliver(0),
        Action::LoseResponse(0),
    ] {
        step(&model, &mut state, action);
    }
    compare(&mut controller, &settings, root.path(), &state);
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .all(|r| r.table != "inbox")
    );

    fault.set(Fault::Pending);
    assert!(controller.drive_once().unwrap());
    step(&model, &mut state, Action::MarkUncertain(0));
    compare(&mut controller, &settings, root.path(), &state);
    let before = controller.snapshot().unwrap();
    assert!(!controller.drive_once().unwrap());
    assert_eq!(
        before,
        controller.snapshot().unwrap(),
        "Pending is a stutter, not absence"
    );
    compare(&mut controller, &settings, root.path(), &state);

    fault.set(Fault::Healthy);
    assert!(controller.drive_once().unwrap());
    step(&model, &mut state, Action::ReadRecord(0));
    step(&model, &mut state, Action::CommitConfirmed(0));
    compare(&mut controller, &settings, root.path(), &state);
    assert_eq!(
        adapters::delivery_counts(root.path()).unwrap()["effect:0:0"],
        1
    );
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "inbox")
    );
}

#[test]
fn real_adapter_epoch_fence_conforms() {
    let root = tempfile::tempdir().unwrap();
    let mut fake = adapters::github::Fake::new(root.path());
    let delivery = |key: usize, epoch: i64| {
        serde_json::from_value::<Delivery>(serde_json::json!({
        "key": format!("s3:{key}"), "epoch": epoch,
        "output": {"Effect": {"node": 0, "seq": key, "kind": "PostApproval", "head": 1}},
        "generation": 1, "head_sha": "abc", "repository": {"owner": "owner", "name": "repo", "installation": 1},
        "issue": 1, "pull_request": 2, "paths": [], "attempt": 1, "round": 0
    })).unwrap()
    };
    let model = EffectModel::new(vec![Contract::Once, Contract::Once]);
    let mut state = model.init_states().remove(0);
    for action in [
        Action::CommitSent(0),
        Action::Dispatch(0),
        Action::CommitSent(1),
        Action::Dispatch(1),
    ] {
        step(&model, &mut state, action);
    }
    fake.execute(&delivery(1, 2)).unwrap();
    step(&model, &mut state, Action::Deliver(1));
    assert!(fake.execute(&delivery(0, 1)).is_err());
    step(&model, &mut state, Action::Deliver(0));
    let counts = adapters::delivery_counts(root.path()).unwrap();
    assert_eq!(counts.len(), 1);
    assert_eq!(counts["s3:1"], u64::from(state.effects[1].mutations));
    assert_eq!(state.effects[0].mutations, 0);
    assert_eq!(state.effects[0].phase, Phase::Fenced);
}

#[tokio::test]
async fn real_comment_reconciliation_conforms_and_keeps_the_last_copy() {
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use wiremock::{
        Mock, MockServer, Request, ResponseTemplate,
        matchers::{method, path},
    };

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/app/installations/1/access_tokens"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "token": "s3-fixture-token", "expires_at": "2099-01-01T00:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let github = tl3_github::GitHub::new(
        tl3_github::AppConfig {
            id: 1,
            slug: "factory".into(),
            pem: include_str!("../../../mergify/tests/fixtures/throwaway-key.pem").into(),
            webhook_secret: "s3-fixture-secret".into(),
        },
        1,
        tl3_github::ClientOptions {
            api_url: format!("{}/", server.uri()).parse().unwrap(),
            mutation_interval: std::time::Duration::ZERO,
            max_retries: 0,
            ..tl3_github::ClientOptions::default()
        },
    )
    .unwrap();
    let comments = Arc::new(Mutex::new(
        (10..=11)
            .map(|id| {
                json!({
                    "id": id, "body": tl3_github::GitHub::marker("s3-message"),
                    "user": {"id": 1, "login": "factory[bot]"},
                    "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
                    "issue_url": "https://api.github.com/repos/owner/repo/issues/1"
                })
            })
            .collect::<Vec<Value>>(),
    ));
    let read = comments.clone();
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/issues/1/comments"))
        .respond_with(move |_: &Request| {
            ResponseTemplate::new(200).set_body_json(read.lock().unwrap().clone())
        })
        .expect(2)
        .mount(&server)
        .await;
    let delete = comments.clone();
    Mock::given(method("DELETE"))
        .and(path("/repos/owner/repo/issues/comments/11"))
        .respond_with(move |_: &Request| {
            delete.lock().unwrap().retain(|c| c["id"] != 11);
            ResponseTemplate::new(204)
        })
        .expect(1)
        .mount(&server)
        .await;

    let model = EffectModel::new(vec![Contract::Message]);
    let mut state = model.init_states().remove(0);
    for action in [
        Action::CommitSent(0),
        Action::Dispatch(0),
        Action::DuplicateDelivery(0),
        Action::Deliver(0),
        Action::Deliver(0),
        Action::LoseResponse(0),
        Action::MarkUncertain(0),
    ] {
        step(&model, &mut state, action);
    }
    assert_eq!(
        comments.lock().unwrap().len(),
        usize::from(state.effects[0].copies)
    );
    let kept = github
        .reconcile_comments("owner/repo", 1, "s3-message")
        .await
        .unwrap()
        .unwrap();
    step(&model, &mut state, Action::ReadRecord(0));
    assert_eq!(kept.id, 10);
    assert_eq!(
        comments.lock().unwrap().len(),
        usize::from(state.effects[0].copies)
    );
    assert_eq!(state.effects[0].copies, 1);
    assert_eq!(
        github
            .reconcile_comments("owner/repo", 1, "s3-message")
            .await
            .unwrap()
            .unwrap()
            .id,
        kept.id
    );
    assert_eq!(comments.lock().unwrap().len(), 1);
}
