#[path = "../../store/tests/support/mod.rs"]
mod database;
#[path = "support/github.rs"]
mod github_fake;

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    executor::{Delivery, Output, Upstream},
};
use tl3_github::{AppConfig, ClientOptions, GitHub, message::Policy};
use tl3_kernel::EffectKind;
use tl3_store::{EffectState, Store};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Default)]
struct Checks {
    held: Option<Request>,
    posts: usize,
    visible: Vec<Value>,
}

impl github_fake::Routes for Checks {
    fn pulls(&self) -> Value {
        json!([])
    }
    fn checks(&self, head: &str) -> Value {
        json!(
            self.visible
                .iter()
                .filter(|check| check["head_sha"] == head)
                .collect::<Vec<_>>()
        )
    }
    fn checks_mut(&mut self) -> Option<&mut Vec<Value>> {
        Some(&mut self.visible)
    }
}

#[derive(Clone, Default)]
struct Api(Arc<Mutex<Checks>>);

impl Api {
    fn release(&self) {
        let mut state = self.0.lock().unwrap();
        let request = state.held.take().expect("first POST is still in flight");
        github_fake::respond(
            &request,
            request.url.path(),
            "/repos/owner/sandbox",
            &mut *state,
        )
        .expect("apply the held POST through the existing GitHub fake");
    }
}

impl Respond for Api {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        if let Some(response) = github_fake::access_token(request.url.path(), 3, "fake-token") {
            return response;
        }
        let mut state = self.0.lock().unwrap();
        if request.method == "POST" && request.url.path() == "/repos/owner/sandbox/check-runs" {
            state.posts += 1;
            if state.posts == 1 {
                // #766: retain the mutation after the client's request deadline.
                // Only release() applies it, after reconciliation and retry.
                state.held = Some(request.clone());
                return ResponseTemplate::new(201).set_delay(Duration::from_secs(60));
            }
        }
        github_fake::respond(
            request,
            request.url.path(),
            "/repos/owner/sandbox",
            &mut *state,
        )
        .expect("unexpected GitHub request")
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
        fail_fast: false,
        operations: None,
    }
}

fn open(settings: &tl3_config::Store, root: &Path, uri: Option<&str>) -> Controller {
    let mut registry = adapters::simulation(&root.join("fakes")).unwrap();
    if let Some(uri) = uri {
        let client = GitHub::new(
            AppConfig {
                id: 123,
                slug: "factory".into(),
                pem: include_str!("../../mergify/tests/fixtures/throwaway-key.pem").into(),
                webhook_secret: "fixture".into(),
            },
            3,
            ClientOptions {
                api_url: format!("{uri}/").parse().unwrap(),
                request_timeout: Duration::from_secs(1),
                mutation_interval: Duration::ZERO,
                max_retries: 0,
                ..ClientOptions::default()
            },
        )
        .unwrap();
        registry.register(
            Upstream::GitHub,
            adapters::github::Adapter::new(
                BTreeMap::from([(3, client)]),
                adapters::workers::FakeWriter,
                0,
                Policy::default(),
                &root.join("github"),
                Some(tl3_config::OperationsIssue {
                    owner: "owner".into(),
                    name: "operations".into(),
                    installation_id: 3,
                    number: 81,
                }),
            )
            .unwrap(),
        );
    }
    Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap()
}

fn ready_approval(controller: &mut Controller) -> String {
    controller
        .intake(LabelledIssue {
            delivery: "approval-race".into(),
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
        for effect in controller.snapshot().unwrap().pending_effects {
            let delivery: Delivery = serde_json::from_value(effect.payload).unwrap();
            if matches!(
                delivery.output,
                Output::Effect {
                    kind: EffectKind::PostApproval,
                    ..
                }
            ) {
                assert_eq!(effect.state, EffectState::Intended);
                return effect.key;
            }
        }
        assert!(
            controller.drive_once().unwrap(),
            "simulation stopped before approval"
        );
    }
    panic!("approval was never intended");
}

/// #766; state-table effects: Approval check is Once per head, and a timed-out
/// request can still land later. GitHub news must not turn an empty read into
/// proof that the earlier POST has ended.
#[test]
fn late_approval_post_after_hold_release_does_not_duplicate() {
    late_post(false);
}

/// #766; state-table effect recovery: Sent -> Uncertain after restart must
/// preserve the original key without assuming a timed-out POST has ended.
#[test]
fn late_approval_post_after_restart_does_not_duplicate() {
    late_post(true);
}

fn late_post(restart: bool) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let api = Api::default();
    let server = runtime.block_on(async {
        let server = MockServer::start().await;
        Mock::given(|_: &Request| true)
            .respond_with(api.clone())
            .mount(&server)
            .await;
        server
    });
    let mut db = database::Database::start();
    let settings = db.create("approval_race");
    let root = tempfile::tempdir().unwrap();
    let mut controller = open(&settings, root.path(), None);
    let key = ready_approval(&mut controller);
    drop(controller);
    let mut controller = open(&settings, root.path(), Some(&server.uri()));
    controller.drive_once().unwrap();
    let effect = Store::open(&settings)
        .unwrap()
        .effect(&key)
        .unwrap()
        .unwrap();
    assert_eq!(effect.state, EffectState::Sent);
    assert_eq!(
        api.0.lock().unwrap().posts,
        1,
        "POST reached the fake before timeout"
    );
    assert!(api.0.lock().unwrap().visible.is_empty());
    controller.drive_once().unwrap();
    assert_eq!(
        Store::open(&settings)
            .unwrap()
            .effect(&key)
            .unwrap()
            .unwrap()
            .state,
        EffectState::Uncertain
    );
    assert!(
        (0..30).any(|_| !controller.drive_once().unwrap()),
        "held controller must settle"
    );
    assert_eq!(
        api.0.lock().unwrap().posts,
        1,
        "transport hold prevents immediate retry"
    );

    if restart {
        drop(controller);
        controller = open(&settings, root.path(), Some(&server.uri()));
    } else {
        controller.retry_held_effects(Upstream::GitHub);
    }
    for _ in 0..10 {
        if !controller.drive_once().unwrap() || api.0.lock().unwrap().posts > 1 {
            break;
        }
    }
    api.release();
    let delivery: Delivery = serde_json::from_value(effect.payload).unwrap();
    let external_id = format!("{key}:{}", delivery.head_sha.unwrap());
    let copies = api
        .0
        .lock()
        .unwrap()
        .visible
        .iter()
        .filter(|check| check["external_id"] == external_id)
        .count();
    assert_eq!(
        copies, 1,
        "late first POST duplicated the retried approval for {external_id}"
    );
    for _ in 0..10 {
        if Store::open(&settings)
            .unwrap()
            .effect(&key)
            .unwrap()
            .unwrap()
            .state
            == EffectState::Confirmed
        {
            assert_eq!(api.0.lock().unwrap().posts, 1);
            return;
        }
        assert!(
            controller.drive_once().unwrap(),
            "late approval must settle the original effect"
        );
    }
    panic!("late approval was never confirmed");
}
