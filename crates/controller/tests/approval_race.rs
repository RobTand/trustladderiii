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
    executor::{
        CommentKind, Completion, Delivery, Executor, Look, Output, Reconciliation, Upstream,
    },
};
use tl3_github::{AppConfig, ClientOptions, GitHub, message::Policy};
use tl3_kernel::EffectKind;
use tl3_store::{ConsoleCommands, EffectState, Store};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Default)]
struct Checks {
    held: Option<Request>,
    posts: usize,
    visible: Vec<Value>,
    refuse_next_read: bool,
    apply_first: bool,
    release_before_retry: bool,
    comments: Vec<Value>,
    fail_comment_reads: bool,
}

impl Checks {
    fn apply(&mut self, request: &Request) -> ResponseTemplate {
        if request.url.path() == "/repos/owner/sandbox/issues/12/comments" {
            if request.method == "GET" {
                if self.fail_comment_reads {
                    return ResponseTemplate::new(503);
                }
                return ResponseTemplate::new(200).set_body_json(json!(self.comments));
            }
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let comment = json!({"id": self.comments.len() + 1, "body": body["body"],
                "user": {"id":99,"login":"factory[bot]"},
                "created_at":"2026-11-04T00:00:00Z", "updated_at":"2026-11-04T00:00:00Z",
                "issue_url":"https://api.github.com/repos/owner/sandbox/issues/12"});
            self.comments.push(comment.clone());
            return ResponseTemplate::new(201).set_body_json(comment);
        }
        if request.method == "DELETE"
            && let Some(id) = request
                .url
                .path()
                .strip_prefix("/repos/owner/sandbox/issues/comments/")
        {
            let id: u64 = id.parse().unwrap();
            self.comments.retain(|comment| comment["id"] != id);
            return ResponseTemplate::new(204);
        }
        github_fake::respond(request, request.url.path(), "/repos/owner/sandbox", self)
            .expect("unexpected GitHub request")
    }
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
        state.apply(&request);
    }
}

impl Respond for Api {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        if let Some(response) = github_fake::access_token(request.url.path(), 3, "fake-token") {
            return response;
        }
        let mut state = self.0.lock().unwrap();
        if request.method == "GET"
            && request.url.path().ends_with("/check-runs")
            && state.refuse_next_read
        {
            state.refuse_next_read = false;
            return ResponseTemplate::new(404);
        }
        if request.method == "POST"
            && matches!(
                request.url.path(),
                "/repos/owner/sandbox/check-runs" | "/repos/owner/sandbox/issues/12/comments"
            )
        {
            state.posts += 1;
            if state.posts == 1 && !state.apply_first {
                // #766: retain the mutation after the client's request deadline.
                // Only release() applies it, after reconciliation and retry.
                state.held = Some(request.clone());
                return ResponseTemplate::new(201).set_delay(Duration::from_secs(60));
            }
            if state.posts == 2 && state.release_before_retry {
                let first = state
                    .held
                    .take()
                    .expect("A remains outstanding until B is sent");
                state.apply(&first);
            }
        }
        state.apply(request)
    }
}

struct TargetGitHub {
    real: adapters::github::Adapter,
    other: adapters::github::Fake,
}

impl TargetGitHub {
    fn target(&mut self, delivery: &Delivery) -> &mut dyn Executor {
        if matches!(
            delivery.output,
            Output::Effect {
                kind: EffectKind::PostApproval,
                ..
            } | Output::Comment {
                kind: CommentKind::ConsoleNote,
                ..
            }
        ) {
            &mut self.real
        } else {
            &mut self.other
        }
    }
}

impl Executor for TargetGitHub {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        self.target(delivery).execute(delivery)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.target(delivery).reconcile(delivery)
    }
    fn look(&mut self, delivery: &Delivery) -> tl3_controller::Result<Look> {
        self.target(delivery).look(delivery)
    }
    fn app_login(&self, installation: u64) -> Option<String> {
        self.real.app_login(installation)
    }
    fn reconcile_message(
        &mut self,
        repository: &Repository,
        issue: u64,
        comment: &tl3_github::Comment,
    ) -> tl3_controller::Result<()> {
        self.real.reconcile_message(repository, issue, comment)
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
            TargetGitHub {
                real: adapters::github::Adapter::new(
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
                other: adapters::github::Fake::new(&root.join("fakes")),
            },
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
    late_post(false, false);
}

/// #766; state-table effect recovery: Sent -> Uncertain after restart must
/// preserve the original key without assuming a timed-out POST has ended.
#[test]
fn late_approval_post_after_restart_does_not_duplicate() {
    late_post(true, false);
}

/// #764/#766: Sent, absent read, A times out, hold, Uncertain, same-version
/// restart, Intended, absent retry read, POST B, then A lands before B.
#[test]
fn approval_spike_schedule_lands_a_then_b_after_restart() {
    late_post(true, true);
}

/// #766; the effect machine permits retry on authoritative absence. A known
/// 404 before POST must retain its existing bounded retry after a successful look.
#[test]
fn refused_approval_read_can_retry_after_an_empty_look() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let api = Api::default();
    {
        let mut state = api.0.lock().unwrap();
        state.refuse_next_read = true;
        state.apply_first = true;
    }
    let server = runtime.block_on(async {
        let server = MockServer::start().await;
        Mock::given(|_: &Request| true)
            .respond_with(api.clone())
            .mount(&server)
            .await;
        server
    });
    let mut db = database::Database::start();
    let settings = db.create("approval_refusal");
    let root = tempfile::tempdir().unwrap();
    let mut controller = open(&settings, root.path(), None);
    let key = ready_approval(&mut controller);
    drop(controller);
    let mut controller = open(&settings, root.path(), Some(&server.uri()));
    assert!((0..30).any(|_| !controller.drive_once().unwrap()));
    assert_eq!(api.0.lock().unwrap().posts, 0);
    drop(controller);
    let mut controller = open(&settings, root.path(), Some(&server.uri()));
    controller.look_key(&key).unwrap();
    assert_eq!(
        controller.world().unwrap().nodes[0].fx_failures,
        1,
        "known refusal must permit the existing bounded retry"
    );
    for _ in 0..20 {
        controller.drive_once().unwrap();
        if !api.0.lock().unwrap().visible.is_empty() {
            assert_eq!(api.0.lock().unwrap().posts, 1);
            return;
        }
    }
    panic!("approval never retried after its known refusal");
}

fn late_post(restart: bool, release_before_retry: bool) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let api = Api::default();
    api.0.lock().unwrap().release_before_retry = release_before_retry;
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
    if api.0.lock().unwrap().held.is_some() {
        api.release();
    }
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

/// #764/#766; state-table comment contract: the next reconciliation deletes
/// extras even when B confirmed the effect before the timed-out A landed.
#[test]
fn late_comment_after_confirmation_is_deduplicated_on_github_news() {
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
    let settings = db.create("comment_race");
    let root = tempfile::tempdir().unwrap();
    let mut controller = open(&settings, root.path(), None);
    controller
        .intake(LabelledIssue {
            delivery: "comment-race".into(),
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
    let id = "00000000-0000-4000-8000-000000000766";
    ConsoleCommands::open(&settings)
        .unwrap()
        .submit(id, 0, &json!({"kind":"pause"}))
        .unwrap();
    tl3_controller::ingress::drain(&mut controller).unwrap();
    let key = format!("console-note:{id}");
    assert_eq!(
        controller.effect(&key).unwrap().unwrap().state,
        EffectState::Intended
    );
    drop(controller);
    let mut controller = open(&settings, root.path(), Some(&server.uri()));
    assert!((0..30).any(|_| !controller.drive_once().unwrap()));
    assert_eq!(api.0.lock().unwrap().posts, 1);
    assert!(api.0.lock().unwrap().comments.is_empty());
    assert_eq!(
        controller.effect(&key).unwrap().unwrap().state,
        EffectState::Uncertain
    );
    drop(controller);
    let mut controller = open(&settings, root.path(), Some(&server.uri()));
    for _ in 0..30 {
        if controller.effect(&key).unwrap().unwrap().state == EffectState::Confirmed {
            break;
        }
        assert!(controller.drive_once().unwrap());
    }
    assert_eq!(
        controller.effect(&key).unwrap().unwrap().state,
        EffectState::Confirmed
    );
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .pending_effects
            .iter()
            .any(|effect| effect.key == key)
    );
    assert_eq!(
        api.0.lock().unwrap().comments.len(),
        1,
        "B landed and confirmed the effect"
    );
    api.release();
    assert_eq!(
        api.0.lock().unwrap().comments.len(),
        2,
        "A landed after confirmation"
    );
    for _ in 0..5 {
        controller.drive_once().unwrap();
    }
    let late = api.0.lock().unwrap().comments.last().unwrap().clone();
    let mut webhook: Value = serde_json::from_str(include_str!(
        "../../github/tests/fixtures/issue-comment-created.json"
    ))
    .unwrap();
    webhook["repository"]["owner"]["login"] = json!("owner");
    webhook["repository"]["name"] = json!("sandbox");
    webhook["installation"]["id"] = json!(3);
    webhook["issue"]["number"] = json!(12);
    webhook["comment"] = late.clone();
    webhook["sender"] = late["user"].clone();
    Store::open(&settings)
        .unwrap()
        .receive_ingress(
            "github-webhook",
            "late-comment",
            "issue_comment",
            &serde_json::to_vec(&webhook).unwrap(),
        )
        .unwrap();
    tl3_controller::ingress::drain(&mut controller).unwrap();
    api.0.lock().unwrap().fail_comment_reads = true;
    assert!((0..30).any(|_| !controller.drive_once().unwrap()));
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "github_message_reconciliations")
    );
    assert_eq!(api.0.lock().unwrap().comments.len(), 2);
    drop(controller);
    api.0.lock().unwrap().fail_comment_reads = false;
    let mut controller = open(&settings, root.path(), Some(&server.uri()));
    assert!((0..30).any(|_| !controller.drive_once().unwrap()));
    assert_eq!(
        api.0.lock().unwrap().comments.len(),
        1,
        "the next reconciliation must clean the late copy of a confirmed comment"
    );
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "github_message_reconciliations")
    );
    assert_eq!(
        controller.effect(&key).unwrap().unwrap().state,
        EffectState::Confirmed
    );
}
