#[path = "support/controller_restart.rs"]
mod controller_restart;
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
    fail_check_reads: bool,
    exhaust_check_quota: bool,
    patches: usize,
    fail_patch_after: Option<usize>,
    apply_first: bool,
    release_before_retry: bool,
    comments: Vec<Value>,
    fail_comment_reads: bool,
    fail_comment_reads_once: bool,
}

impl Checks {
    fn apply(&mut self, request: &Request) -> ResponseTemplate {
        if request.method == "GET" && request.url.path() == "/repos/owner/sandbox/issues/12" {
            return ResponseTemplate::new(200).set_body_json(json!({"number":12,"title":"request","body":"request","state":"open","updated_at":"2026-11-04T00:00:00Z","labels":[],"pull_request":null}));
        }
        if request.url.path() == "/repos/owner/sandbox/issues/12/comments" {
            if request.method == "GET" {
                if std::mem::take(&mut self.fail_comment_reads_once) || self.fail_comment_reads {
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
        if request.method == "PATCH" && request.url.path().contains("/check-runs/") {
            if self.fail_patch_after == Some(self.patches) {
                return ResponseTemplate::new(503);
            }
            let id: u64 = request
                .url
                .path()
                .rsplit('/')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let check = self.visible.iter_mut().find(|c| c["id"] == id).unwrap();
            for (key, value) in body.as_object().unwrap() {
                check[key] = value.clone();
            }
            let response = check.clone();
            self.patches += 1;
            return ResponseTemplate::new(200).set_body_json(response);
        }
        if request.method == "GET"
            && matches!(
                request.url.path(),
                "/repos/owner/sandbox/issues"
                    | "/repos/owner/sandbox/issues/comments"
                    | "/repos/owner/sandbox/issues/events"
            )
        {
            return ResponseTemplate::new(200).set_body_json(json!([]));
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
            && state.fail_check_reads
        {
            return ResponseTemplate::new(503);
        }
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
        let response = state.apply(request);
        if state.exhaust_check_quota
            && request.method == "GET"
            && request.url.path().ends_with("/check-runs")
        {
            return response
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("retry-after", "120");
        }
        response
    }
}

#[derive(Clone)]
struct ConcurrentApi {
    api: Api,
    reads: Arc<std::sync::Barrier>,
}
impl Respond for ConcurrentApi {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        // Each HTTP frontend has its own wiremock server thread. Both capture
        // the same stale listing before either reconciler can issue a PATCH.
        let response = self.api.respond(request);
        if request.method == "GET" && request.url.path().ends_with("/check-runs") {
            self.reads.wait();
        }
        response
    }
}

struct TargetGitHub {
    real: adapters::github::Adapter,
    other: adapters::github::Fake,
    legacy: bool,
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
        let result = self.target(delivery).execute(delivery);
        if self.legacy && matches!(result, Err(tl3_controller::Error::ApprovalAmbiguous)) {
            return Err(tl3_github::Error::Transport.into());
        }
        result
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.target(delivery).reconcile(delivery)
    }
    fn repair_approval(
        &mut self,
        delivery: &Delivery,
    ) -> tl3_controller::Result<(Reconciliation, Option<usize>)> {
        self.target(delivery).repair_approval(delivery)
    }
    fn reconcile_cancellation(
        &mut self,
        delivery: &Delivery,
    ) -> tl3_controller::Result<Reconciliation> {
        self.target(delivery).reconcile_cancellation(delivery)
    }
    fn retire_delivery(&mut self, delivery: &Delivery) -> tl3_controller::Result<()> {
        self.target(delivery).retire_delivery(delivery)
    }
    fn look(&mut self, delivery: &Delivery) -> tl3_controller::Result<Look> {
        let result = self.target(delivery).look(delivery);
        // The pre-#766 adapter returned Absent after an empty HTTP listing,
        // without a durable attempt record. Exercise the actual allowance retry.
        if self.legacy && matches!(result, Ok(Look::Pending)) {
            return Ok(Look::Absent);
        }
        result
    }
    fn probe(&mut self, delivery: &Delivery) -> tl3_controller::Result<bool> {
        self.real.probe(delivery)
    }
    fn retire(&mut self, store: &mut Store) -> tl3_controller::Result<()> {
        self.real.retire(store)
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
    open_mode(settings, root, uri, false)
}
fn open_mode(
    settings: &tl3_config::Store,
    root: &Path,
    uri: Option<&str>,
    legacy: bool,
) -> Controller {
    open_clock(
        settings,
        root,
        uri,
        legacy,
        tl3_controller::clock::SystemClock,
    )
}
fn open_clock(
    settings: &tl3_config::Store,
    root: &Path,
    uri: Option<&str>,
    legacy: bool,
    clock: impl tl3_controller::clock::Clock + 'static,
) -> Controller {
    // #785: dropping the client does not synchronously reap its server session.
    controller_restart::wait_for_owner_release(&mut fixture_connection(settings));
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
                legacy,
                real: adapters::github::Adapter::new(
                    BTreeMap::from([(3, client)]),
                    adapters::workers::FakeWriter,
                    Store::open(settings).unwrap().load().unwrap().epoch.get(),
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
    Controller::open_with_clock(Store::open(settings).unwrap(), registry, bootstrap(), clock)
        .unwrap()
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
    if let Some(incident) = controller.snapshot().unwrap().rows.iter().find(|r| {
        r.table == "incidents"
            && r.value["open"] == true
            && r.value["signature"]["class"] == "message_cleanup"
    }) {
        controller
            .observe(
                "clear-restarted-cleanup",
                tl3_controller::executor::Observation::GitHubComment {
                    repository: bootstrap().repos[0].clone(),
                    issue: 12,
                    comment: tl3_github::Comment {
                        id: 902,
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
    }
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

struct Case {
    _database: database::Database,
    settings: tl3_config::Store,
    root: tempfile::TempDir,
    server: MockServer,
    api: Api,
    key: String,
    _runtime: tokio::runtime::Runtime,
}

impl Case {
    fn new(name: &str) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
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
        let mut database = database::Database::start();
        let settings = database.create(name);
        let root = tempfile::tempdir().unwrap();
        let mut controller = open(&settings, root.path(), None);
        let key = ready_approval(&mut controller);
        drop(controller);
        Self {
            _runtime: runtime,
            _database: database,
            settings,
            root,
            server,
            api,
            key,
        }
    }
    fn open(&self) -> Controller {
        open(&self.settings, self.root.path(), Some(&self.server.uri()))
    }
    fn delivery(&self) -> Delivery {
        serde_json::from_value(
            Store::open(&self.settings)
                .unwrap()
                .effect(&self.key)
                .unwrap()
                .unwrap()
                .payload,
        )
        .unwrap()
    }
    fn incident(&self, controller: &mut Controller) -> Option<String> {
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .find(|r| {
                r.table == "incidents" && r.value["open"] == true && r.value["effect"] == self.key
            })
            .map(|r| r.id.clone())
    }
    fn timeout(&self) -> Controller {
        let mut controller = self.open();
        controller.drive_once().unwrap();
        drain(&mut controller);
        assert_eq!(self.api.0.lock().unwrap().posts, 1);
        controller
    }
    fn clear(&self, controller: &mut Controller) {
        let id = self
            .incident(controller)
            .expect("dispatch ambiguity must raise a visible incident");
        controller
            .observe(
                "human-clear",
                tl3_controller::executor::Observation::GitHubComment {
                    repository: bootstrap().repos[0].clone(),
                    issue: 12,
                    comment: tl3_github::Comment {
                        id: 900,
                        body: format!("clear incident {id}"),
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
        drain(controller);
    }
    fn check_news(&self, controller: &mut Controller, check: Value) {
        let webhook = json!({"action":"completed", "check_run":check, "installation":{"id":3}, "repository":{"id":1,"name":"sandbox","owner":{"id":42,"login":"owner"}}, "sender":{"id":99,"login":"factory[bot]"}});
        Store::open(&self.settings)
            .unwrap()
            .receive_ingress(
                "github-webhook",
                "late-approval",
                "check_run",
                &serde_json::to_vec(&webhook).unwrap(),
            )
            .unwrap();
        tl3_controller::ingress::drain(controller).unwrap();
    }
    fn corrupt_copies(&self, count: usize) {
        let delivery = self.delivery();
        let mut api = self.api.0.lock().unwrap();
        api.visible = (0..count).map(|i| json!({"id":i+1,"name":"TL3 approval","head_sha":delivery.head_sha,"external_id":format!("{}:{}",self.key,delivery.head_sha.as_ref().unwrap()),"app":{"id":123,"slug":"factory"},"status":"completed","conclusion":if i==0 {"success"} else {"failure"},"output":{"title":"stale","summary":"stale","text":"stale"}})).collect();
    }
    fn assert_converged(&self, count: usize) {
        let state = self.api.0.lock().unwrap();
        assert_eq!(state.visible.len(), count);
        for check in &state.visible {
            assert_eq!(
                check["conclusion"], "success",
                "every copy must preserve the effective approval"
            );
            assert_eq!(check["status"], "completed");
            assert_eq!(
                check["output"], state.visible[0]["output"],
                "all copies need identical output"
            );
            assert_ne!(
                check["output"]["title"], "stale",
                "reconcile from the intended approval, not an arbitrary copy"
            );
        }
    }
}

fn drain(controller: &mut Controller) {
    assert!(
        (0..100).any(|_| !controller.drive_once().unwrap()),
        "controller must settle"
    );
}

/// #766, Q27: legacy Sent -> Intended before upgrade does not authorize a retry.
#[test]
fn legacy_retry_intention_after_upgrade_is_ambiguous() {
    let case = Case::new("legacy_upgrade");
    drop(case.timeout());
    let mut store = Store::open(&case.settings).unwrap();
    let snapshot = store.load().unwrap();
    let mut commit = tl3_store::Commit::new(
        "old-version-empty-read",
        json!({"effect":case.key,"from":"Uncertain","to":"Intended"}),
        &snapshot,
    );
    commit.transitions.push(tl3_store::EffectTransition {
        key: case.key.clone(),
        from: EffectState::Uncertain,
        to: EffectState::Intended,
    });
    // Simulate exactly the older version's durable state, without its in-memory hold.
    for row in &snapshot.rows {
        if row.table == "incidents" {
            commit.rows.push(tl3_store::RowChange::Delete {
                table: row.table.clone(),
                id: row.id.clone(),
            });
        }
    }
    store.commit(&commit).unwrap();
    for entry in std::fs::read_dir(case.root.path().join("github")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "approval") {
            std::fs::remove_file(path).unwrap();
        }
    }
    let snapshot = store.load().unwrap();
    let mut upgrade = tl3_store::Commit::new("upgrade", json!({"upgrade":true}), &snapshot);
    upgrade.next_epoch = Some(tl3_store::Epoch::try_from(snapshot.epoch.get() + 1).unwrap());
    store.commit(&upgrade).unwrap();
    drop(store);
    let mut controller = case.open();
    drain(&mut controller);
    assert_eq!(
        case.api.0.lock().unwrap().posts,
        1,
        "legacy retry must not send B after re-epoching"
    );
    assert!(case.incident(&mut controller).is_some());
}

/// #766, state-table delivery failures: GET failures before POST are ordinary retries.
#[test]
fn preflight_failure_never_strands_an_unsent_approval() {
    let case = Case::new("preflight");
    case.api.0.lock().unwrap().fail_check_reads = true;
    let mut controller = case.open();
    drain(&mut controller);
    assert_eq!(case.api.0.lock().unwrap().posts, 0);
    for entry in std::fs::read_dir(case.root.path().join("github")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "approval") {
            assert_ne!(
                std::fs::read_to_string(path).unwrap(),
                "\"Uncertain\"",
                "a preflight failure has no outstanding POST"
            );
        }
    }
    drop(controller);
    let mut controller = case.open();
    drain(&mut controller);
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "incidents"
                && row.value["open"] == true
                && row.value["signature"]["class"] == "approval_outcome_unknown"),
        "restarting while GET still fails cannot invent a dispatched POST"
    );
    drop(controller);
    {
        let mut api = case.api.0.lock().unwrap();
        api.fail_check_reads = false;
        api.apply_first = true;
    }
    let mut controller = case.open();
    controller.look_key(&case.key).unwrap();
    drain(&mut controller);
    assert_eq!(
        case.api.0.lock().unwrap().posts,
        1,
        "a recovered GET must allow an ordinary retry"
    );
}

/// #766, row 670: matching upstream evidence resolves the visible ambiguity incident.
#[test]
fn ambiguous_approval_incident_resolves_on_matching_evidence() {
    let case = Case::new("ambiguity");
    let mut controller = case.timeout();
    assert!(
        case.incident(&mut controller).is_some(),
        "unknown POST outcome must be visible"
    );
    case.api.release();
    drain(&mut controller);
    assert_eq!(
        controller.effect(&case.key).unwrap().unwrap().state,
        EffectState::Confirmed
    );
    assert!(case.incident(&mut controller).is_none());
}

/// #766, Q27 approved 2026-11-04: clear authorizes B, late A converges after confirmation.
#[test]
fn cleared_retry_then_late_a_converges_to_one_effective_approval() {
    let case = Case::new("human_retry");
    let mut controller = case.timeout();
    case.clear(&mut controller);
    assert_eq!(
        case.api.0.lock().unwrap().posts,
        2,
        "only human clear authorizes B"
    );
    assert_eq!(
        controller.effect(&case.key).unwrap().unwrap().state,
        EffectState::Confirmed
    );
    case.api.release();
    case.corrupt_copies(2);
    let late = case.api.0.lock().unwrap().visible[1].clone();
    case.check_news(&mut controller, late);
    drain(&mut controller);
    case.assert_converged(2);
}

/// #766, effect cancellation: stop retires ambiguity without any new POST; epoch can advance.
#[test]
fn stop_retires_ambiguous_approval_and_allows_upgrade() {
    let case = Case::new("stop_approval");
    let mut controller = case.timeout();
    let node = case.delivery().node();
    controller
        .observe(
            "stop",
            tl3_controller::executor::Observation::Kernel(tl3_kernel::Event::Command {
                node,
                id: 77,
                command: tl3_kernel::Command::Stop,
            }),
        )
        .unwrap();
    drain(&mut controller);
    assert_eq!(
        controller.effect(&case.key).unwrap().unwrap().state,
        EffectState::Cancelled,
        "stop must retire the ambiguous attempt"
    );
    assert_eq!(case.api.0.lock().unwrap().posts, 1);
    drop(controller);
    let mut store = Store::open(&case.settings).unwrap();
    let snapshot = store.load().unwrap();
    let mut upgrade =
        tl3_store::Commit::new("upgrade-after-stop", json!({"upgrade":true}), &snapshot);
    upgrade.next_epoch = Some(tl3_store::Epoch::try_from(snapshot.epoch.get() + 1).unwrap());
    store.commit(&upgrade).unwrap();
}

/// #766, bounded executor state: no terminal delivery retains an .approval attempt file.
#[test]
fn terminal_approval_attempt_files_are_retired() {
    let case = Case::new("retire_approval");
    let mut controller = case.timeout();
    case.api.release();
    drain(&mut controller);
    assert_eq!(
        controller.effect(&case.key).unwrap().unwrap().state,
        EffectState::Confirmed
    );
    let attempts = std::fs::read_dir(case.root.path().join("github"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|e| e == "approval")
        })
        .count();
    assert_eq!(
        attempts, 0,
        "terminal records must be bounded by pending deliveries"
    );
}

/// #766, Q27 convergence: partial PATCH progress survives controller restart.
#[test]
fn restart_midway_through_approval_reconciliation_converges() {
    let case = Case::new("restart_repair");
    let mut controller = case.timeout();
    case.api.release();
    drain(&mut controller);
    case.corrupt_copies(3);
    case.api.0.lock().unwrap().fail_patch_after = Some(1);
    let late = case.api.0.lock().unwrap().visible[2].clone();
    case.check_news(&mut controller, late);
    drain(&mut controller);
    assert_eq!(
        case.api.0.lock().unwrap().patches,
        1,
        "the first PATCH must land before restart"
    );
    drop(controller);
    case.api.0.lock().unwrap().fail_patch_after = None;
    let mut controller = case.open();
    controller.look_now().unwrap();
    drain(&mut controller);
    case.assert_converged(3);
}

/// #766, Q27 convergence: two reconcilers read the same copies and update idempotently.
#[test]
fn concurrent_approval_reconciliations_converge() {
    let case = Case::new("parallel_repair");
    case.corrupt_copies(3);
    let delivery = case.delivery();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let reads = Arc::new(std::sync::Barrier::new(2));
    let servers: Vec<_> = (0..2)
        .map(|_| {
            case._runtime.block_on(async {
                let server = MockServer::start().await;
                Mock::given(|_: &Request| true)
                    .respond_with(ConcurrentApi {
                        api: case.api.clone(),
                        reads: reads.clone(),
                    })
                    .mount(&server)
                    .await;
                server
            })
        })
        .collect();
    std::thread::scope(|scope| {
        for (index, server) in servers.iter().enumerate() {
            let barrier = barrier.clone();
            let delivery = delivery.clone();
            let uri = server.uri();
            let root = case.root.path().join(format!("reconciler-{index}"));
            scope.spawn(move || {
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
                        mutation_interval: Duration::ZERO,
                        max_retries: 0,
                        ..ClientOptions::default()
                    },
                )
                .unwrap();
                let mut adapter = adapters::github::Adapter::new(
                    BTreeMap::from([(3, client)]),
                    adapters::workers::FakeWriter,
                    0,
                    Policy::default(),
                    &root,
                    Some(tl3_config::OperationsIssue {
                        owner: "owner".into(),
                        name: "operations".into(),
                        installation_id: 3,
                        number: 81,
                    }),
                )
                .unwrap();
                barrier.wait();
                assert!(matches!(
                    adapter.reconcile(&delivery).unwrap(),
                    Reconciliation::Found(_)
                ));
            });
        }
    });
    case.assert_converged(3);
}

fn queue_duplicate_comments(case: &Case, controller: &mut Controller) {
    let body = format!("note\n\n{}", GitHub::marker(&case.key));
    for id in 1..=2 {
        let value = json!({"id":id,"body":body,"user":{"id":99,"login":"factory[bot]"},"created_at":"2026-11-04T00:00:00Z","updated_at":"2026-11-04T00:00:00Z","issue_url":"https://api.github.com/repos/owner/sandbox/issues/12"});
        case.api.0.lock().unwrap().comments.push(value.clone());
        controller
            .observe(
                &format!("comment-{id}"),
                tl3_controller::executor::Observation::GitHubComment {
                    repository: bootstrap().repos[0].clone(),
                    issue: 12,
                    comment: serde_json::from_value(value).unwrap(),
                },
            )
            .unwrap();
    }
}

/// #766, state-table comment deduplication: one reconciliation per issue and effect key.
#[test]
fn cleanup_is_coalesced_by_issue_and_effect_key() {
    let case = Case::new("cleanup_coalesce");
    let mut controller = case.open();
    queue_duplicate_comments(&case, &mut controller);
    assert_eq!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.table == "github_message_reconciliations")
            .count(),
        1,
        "two webhook copies are one cleanup operation"
    );
}

/// #766, state-table:307: every second retryable cleanup failure raises an incident.
#[test]
fn cleanup_first_incident_recovers_on_linked_probe() {
    let case = Case::new("cleanup_incident");
    let mut controller = case.open();
    queue_duplicate_comments(&case, &mut controller);
    case.api.0.lock().unwrap().fail_comment_reads = true;
    controller.drive_once().unwrap();
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents" && r.value["signature"]["class"] == "message_cleanup")
    );
    controller.drive_once().unwrap();
    let snapshot = controller.snapshot().unwrap();
    let incident = snapshot
        .rows
        .iter()
        .find(|r| {
            r.table == "incidents"
                && r.value["open"] == true
                && r.value["signature"]["class"] == "message_cleanup"
        })
        .expect("second retryable cleanup failure must raise an incident");
    let id = incident.id.clone();
    case.api.0.lock().unwrap().fail_comment_reads = false;
    controller.look_now().unwrap();
    drain(&mut controller);
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents" && r.id == id && r.value["open"] == true),
        "the first cleanup incident must clear on its successful linked probe"
    );
    drain(&mut controller);
    assert_eq!(case.api.0.lock().unwrap().comments.len(), 1);
    assert_eq!(case.api.0.lock().unwrap().comments[0]["id"], 1);
}

/// #766, Q27: catch-up news repairs terminal approvals when a late webhook was missed.
#[test]
fn catchup_repairs_approval_copies_after_confirmation() {
    let case = Case::new("approval_catchup");
    let mut controller = case.timeout();
    case.api.release();
    drain(&mut controller);
    assert_eq!(
        controller.effect(&case.key).unwrap().unwrap().state,
        EffectState::Confirmed
    );
    case.corrupt_copies(2);
    let delivery = tl3_controller::ingress::GitHubDelivery::CatchUp {
        repository: bootstrap().repos[0].clone(),
        changes: Box::new(tl3_github::RepoChanges {
            repository: "owner/sandbox".into(),
            issues: vec![],
            comments: vec![],
            labels: vec![],
            closed: vec![],
            pull_requests: vec![],
            check_suites: vec![],
            check_runs: case
                .api
                .0
                .lock()
                .unwrap()
                .visible
                .iter()
                .cloned()
                .map(|c| serde_json::from_value(c).unwrap())
                .collect(),
        }),
    };
    Store::open(&case.settings)
        .unwrap()
        .receive_ingress(
            "github-catchup",
            "late-check-catchup",
            "catch-up",
            &serde_json::to_vec(&delivery).unwrap(),
        )
        .unwrap();
    tl3_controller::ingress::drain(&mut controller).unwrap();
    drain(&mut controller);
    case.assert_converged(2);
}

/// #766, Q27: a legacy allowance creates B; A's unknown dispatch follows it across upgrade.
#[test]
fn legacy_allowance_replacement_after_upgrade_requires_authorization() {
    let case = Case::new("legacy_replacement");
    let mut controller = open_mode(
        &case.settings,
        case.root.path(),
        Some(&case.server.uri()),
        true,
    );
    drain(&mut controller);
    assert_eq!(case.api.0.lock().unwrap().posts, 1);
    controller.look_key(&case.key).unwrap();
    assert_eq!(
        controller.effect(&case.key).unwrap().unwrap().state,
        EffectState::Cancelled
    );
    let replacement = controller
        .snapshot()
        .unwrap()
        .pending_effects
        .iter()
        .find(|e| {
            let d: Delivery = serde_json::from_value(e.payload.clone()).unwrap();
            matches!(
                d.output,
                Output::Effect {
                    kind: EffectKind::PostApproval,
                    ..
                }
            )
        })
        .unwrap()
        .key
        .clone();
    assert_ne!(
        replacement, case.key,
        "the real legacy Ack::Retryable must allocate a new key"
    );
    drop(controller);
    let mut store = Store::open(&case.settings).unwrap();
    let snapshot = store.load().unwrap();
    let mut upgrade = tl3_store::Commit::new(
        "legacy-upgrade-replacement",
        json!({"upgrade":true}),
        &snapshot,
    );
    upgrade.next_epoch = Some(tl3_store::Epoch::try_from(snapshot.epoch.get() + 1).unwrap());
    // Old versions did not write any new approval policy metadata.
    for row in &snapshot.rows {
        if row.table.starts_with("approval_") || row.table == "incidents" {
            upgrade.rows.push(tl3_store::RowChange::Delete {
                table: row.table.clone(),
                id: row.id.clone(),
            });
        }
    }
    store.commit(&upgrade).unwrap();
    drop(store);
    for entry in std::fs::read_dir(case.root.path().join("github")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "approval") {
            std::fs::remove_file(path).unwrap();
        }
    }
    let mut controller = case.open();
    drain(&mut controller);
    assert_eq!(
        case.api.0.lock().unwrap().posts,
        1,
        "B must inherit A's dispatch ambiguity before any new POST"
    );
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents"
                && r.value["effect"] == replacement
                && r.value["open"] == true)
    );
}

/// #766, delivery failures: preflight can exhaust quota without sending the POST.
#[test]
fn quota_exhausted_by_preflight_is_not_dispatch_ambiguity() {
    let case = Case::new("preflight_last_quota");
    case.api.0.lock().unwrap().exhaust_check_quota = true;
    let clock = ManualClock(Arc::new(std::sync::atomic::AtomicU64::new(1_000_000)));
    let mut controller = open_clock(
        &case.settings,
        case.root.path(),
        Some(&case.server.uri()),
        false,
        clock.clone(),
    );
    drain(&mut controller);
    assert_eq!(case.api.0.lock().unwrap().posts, 0);
    assert!(
        case.incident(&mut controller).is_none(),
        "local quota refusal did not send a POST and must not require human authorization"
    );
    drop(controller);
    {
        let mut api = case.api.0.lock().unwrap();
        api.exhaust_check_quota = false;
        api.apply_first = true;
    }
    clock
        .0
        .fetch_add(121_000, std::sync::atomic::Ordering::Relaxed);
    let mut controller = open_clock(
        &case.settings,
        case.root.path(),
        Some(&case.server.uri()),
        false,
        clock,
    );
    controller.look_key(&case.key).unwrap();
    drain(&mut controller);
    assert_eq!(case.api.0.lock().unwrap().posts, 1);
}

/// #766, state-table:307: failed terminal PATCHes retain a bounded retry and linked probe.
#[test]
fn terminal_repair_failure_has_incident_and_timer_recovery() {
    let case = Case::new("repair_timer");
    case.api.0.lock().unwrap().apply_first = true;
    let mut controller = case.open();
    drain(&mut controller);
    case.corrupt_copies(3);
    case.api.0.lock().unwrap().fail_patch_after = Some(1);
    case.check_news(
        &mut controller,
        case.api.0.lock().unwrap().visible[2].clone(),
    );
    drain(&mut controller);
    let snapshot = controller.snapshot().unwrap();
    let incident = snapshot
        .rows
        .iter()
        .find(|r| {
            r.table == "incidents"
                && r.value["signature"]["class"] == "approval_repair"
                && r.value["open"] == true
        })
        .expect("two failed terminal repairs must raise a visible incident");
    assert!(
        snapshot
            .rows
            .iter()
            .any(|r| r.table == "allowances" && r.value["incident"] == incident.id),
        "repair must have an existing-timer recovery wake"
    );
    assert!(controller.next_look_in().unwrap().is_some());
    drop(controller);
    case.api.0.lock().unwrap().fail_patch_after = None;
    let mut controller = case.open();
    controller.look_now().unwrap();
    drain(&mut controller);
    case.assert_converged(3);
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "approval_reconciliations")
    );
}

/// #766: retired effect history must never require a historical adapter at startup.
#[test]
fn retired_approval_history_does_not_block_open_without_adapter() {
    let case = Case::new("retired_no_adapter");
    case.api.0.lock().unwrap().apply_first = true;
    let mut controller = case.open();
    drain(&mut controller);
    assert_eq!(
        controller.effect(&case.key).unwrap().unwrap().state,
        EffectState::Confirmed
    );
    drop(controller);
    let result = Controller::open(
        Store::open(&case.settings).unwrap(),
        tl3_controller::executor::Registry::default(),
        bootstrap(),
    );
    assert!(
        result.is_ok(),
        "retired history must not require its old GitHub adapter"
    );
}

#[derive(Clone)]
struct ManualClock(Arc<std::sync::atomic::AtomicU64>);
impl tl3_controller::clock::Clock for ManualClock {
    fn now(&self) -> std::time::SystemTime {
        std::time::UNIX_EPOCH
            + Duration::from_millis(self.0.load(std::sync::atomic::Ordering::Relaxed))
    }
}

/// #766, state-table:307: a later cleanup incident requires Rob, despite healthy probes.
#[test]
fn cleanup_second_incident_requires_rob() {
    let case = Case::new("cleanup_second_incident");
    let mut controller = case.open();
    queue_duplicate_comments(&case, &mut controller);
    case.api.0.lock().unwrap().fail_comment_reads = true;
    drain(&mut controller);
    controller.look_now().unwrap(); // The issue GET probe succeeds; cleanup itself still fails.
    drain(&mut controller);
    let incident = controller
        .snapshot()
        .unwrap()
        .rows
        .iter()
        .find(|r| {
            r.table == "incidents"
                && r.value["open"] == true
                && r.value["signature"]["class"] == "message_cleanup"
        })
        .unwrap()
        .clone();
    assert_eq!(incident.value["needs_rob"], true);
    case.api.0.lock().unwrap().fail_comment_reads = false;
    controller.look_now().unwrap();
    drain(&mut controller);
    assert_eq!(
        case.api.0.lock().unwrap().comments.len(),
        2,
        "a healthy probe cannot clear a later incident"
    );
    controller
        .observe(
            "clear-second-cleanup",
            tl3_controller::executor::Observation::GitHubComment {
                repository: bootstrap().repos[0].clone(),
                issue: 12,
                comment: tl3_github::Comment {
                    id: 999,
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
    drain(&mut controller);
    assert_eq!(case.api.0.lock().unwrap().comments.len(), 1);
}

/// #766, Q27: terminal Pending repair also schedules recovery, without a webhook or manual look.
#[test]
fn pending_terminal_repair_recovers_on_due_timer_after_restart() {
    let case = Case::new("repair_pending_timer");
    let clock = ManualClock(Arc::new(std::sync::atomic::AtomicU64::new(1_000_000)));
    case.api.0.lock().unwrap().apply_first = true;
    let mut controller = open_clock(
        &case.settings,
        case.root.path(),
        Some(&case.server.uri()),
        false,
        clock.clone(),
    );
    drain(&mut controller);
    let check = case.api.0.lock().unwrap().visible[0].clone();
    case.api.0.lock().unwrap().visible.clear();
    case.check_news(&mut controller, check);
    drain(&mut controller);
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents"
                && r.value["signature"]["class"] == "approval_repair"
                && r.value["open"] == true)
    );
    drop(controller);
    case.corrupt_copies(3);
    clock
        .0
        .fetch_add(31_000, std::sync::atomic::Ordering::Relaxed);
    let mut controller = open_clock(
        &case.settings,
        case.root.path(),
        Some(&case.server.uri()),
        false,
        clock,
    );
    controller.look_due().unwrap();
    drain(&mut controller);
    case.assert_converged(3);
    let snapshot = controller.snapshot().unwrap();
    assert!(
        !snapshot
            .rows
            .iter()
            .any(|r| r.table == "approval_reconciliations"
                || (r.table == "allowances" && r.id.starts_with("approval-repair:"))
                || (r.table == "maintenance_failures"))
    );
}

/// #766, delivery failures: connect refusal before writing a POST is ordinary retryable failure.
#[test]
fn connect_failure_before_post_has_no_dispatch_evidence() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let uri = format!("http://{}/", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for body in [
            json!({"token":"fake-token", "expires_at":"2099-01-01T00:00:00Z"}),
            json!({"total_count":0,"check_runs":[]}),
        ] {
            let (mut socket, _) = listener.accept().unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                headers.push(byte[0]);
            }
            let body = body.to_string();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
        // Joining this thread closes the listener before the POST starts.
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let client = GitHub::new(
        AppConfig {
            id: 123,
            slug: "factory".into(),
            pem: include_str!("../../mergify/tests/fixtures/throwaway-key.pem").into(),
            webhook_secret: "fixture".into(),
        },
        3,
        ClientOptions {
            api_url: uri.parse().unwrap(),
            max_retries: 0,
            mutation_interval: Duration::ZERO,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    runtime
        .block_on(client.check_runs("owner/sandbox", "head"))
        .unwrap();
    server.join().unwrap();
    let failure = runtime
        .block_on(client.post_check_run_tracked(
            "owner/sandbox",
            &tl3_github::NewCheckRun {
                name: "TL3 approval".into(),
                head_sha: "head".into(),
                external_id: "key:head".into(),
                status: tl3_github::CheckStatus::Completed,
                conclusion: Some(tl3_github::CheckConclusion::Success),
            },
            None,
        ))
        .unwrap_err();
    assert!(
        !failure.sent,
        "a connection refused before send cannot be an outstanding POST"
    );
    assert!(matches!(failure.error, tl3_github::Error::Transport));
}

/// #766, state-table:307/:272: absent historical effects retain two-failure/probe recovery.
#[test]
fn missing_effect_cleanup_recovers_without_rob() {
    let case = Case::new("cleanup_missing_effect");
    let mut controller = case.open();
    queue_duplicate_comments(&case, &mut controller);
    // The upstream marker outlives its local effect. Do not delete immutable audit data.
    let mut store = Store::open(&case.settings).unwrap();
    let snapshot = store.load().unwrap();
    let mut change = tl3_store::Commit::new("orphan-cleanup", json!(null), &snapshot);
    for row in snapshot
        .rows
        .iter()
        .filter(|r| r.table == "github_message_reconciliations")
    {
        let mut value = row.value.clone();
        value["GitHubComment"]["comment"]["body"] =
            json!(format!("note\n\n{}", GitHub::marker("missing-history")));
        change.rows.push(tl3_store::RowChange::Put {
            table: row.table.clone(),
            id: row.id.clone(),
            value,
        });
    }
    store.commit(&change).unwrap();
    for comment in &mut case.api.0.lock().unwrap().comments {
        comment["body"] = json!(format!("note\n\n{}", GitHub::marker("missing-history")));
    }
    assert!(store.effect("missing-history").unwrap().is_none());
    case.api.0.lock().unwrap().fail_comment_reads_once = true;
    // First drive coalesces the legacy work key, second makes the sole failing GET.
    controller.drive_once().unwrap();
    controller.drive_once().unwrap();
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents" && r.value["open"] == true),
        "one transient orphan cleanup failure must not open an incident"
    );
    controller.look_now().unwrap();
    drain(&mut controller);
    assert_eq!(
        case.api.0.lock().unwrap().comments.len(),
        1,
        "orphan cleanup must recover without Rob"
    );
    // Requeue the same missing-effect marker and prove the first two-failure
    // incident has the linked probe even though no historical delivery exists.
    let mut duplicate = case.api.0.lock().unwrap().comments[0].clone();
    duplicate["id"] = json!(3);
    case.api.0.lock().unwrap().comments.push(duplicate.clone());
    controller
        .observe(
            "orphan-copy-again",
            tl3_controller::executor::Observation::GitHubComment {
                repository: bootstrap().repos[0].clone(),
                issue: 12,
                comment: serde_json::from_value(duplicate).unwrap(),
            },
        )
        .unwrap();
    case.api.0.lock().unwrap().fail_comment_reads = true;
    controller.drive_once().unwrap();
    controller.drive_once().unwrap();
    let incident = controller
        .snapshot()
        .unwrap()
        .rows
        .iter()
        .find(|r| {
            r.table == "incidents"
                && r.value["open"] == true
                && r.value["effect"]
                    .as_str()
                    .is_some_and(|key| key.starts_with("github-cleanup:"))
        })
        .unwrap()
        .id
        .clone();
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "allowances"
                && r.value["maintenance"] == true
                && r.value["incident"] == incident)
    );
    case.api.0.lock().unwrap().fail_comment_reads = false;
    controller.look_now().unwrap();
    drain(&mut controller);
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents" && r.id == incident && r.value["open"] == true)
    );
    assert_eq!(case.api.0.lock().unwrap().comments.len(), 1);
}

/// #766, Q27: a real gap catch-up must find closed-head late POSTs without a webhook.
#[test]
fn gap_catchup_repairs_late_approval_after_closure() {
    let case = Case::new("closed_gap_approval");
    let mut controller = case.timeout();
    case.clear(&mut controller); // B lands; A is still outstanding beyond its timeout.
    controller
        .observe(
            "stop-before-late-a",
            tl3_controller::executor::Observation::Kernel(tl3_kernel::Event::Command {
                node: case.delivery().node(),
                id: 171,
                command: tl3_kernel::Command::Stop,
            }),
        )
        .unwrap();
    drain(&mut controller);
    assert!(matches!(
        controller.world().unwrap().nodes[0].phase,
        tl3_kernel::Phase::Closed(_)
    ));
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "allowances"),
        "closed requests have no active delivery allowances"
    );
    drop(controller);
    let mut controller = case.open();
    // The retained legacy request carries its old serialized output. Apply the real
    // outstanding POST, rather than injecting check runs or a catch-up payload.
    {
        let mut api = case.api.0.lock().unwrap();
        let request = api.held.as_mut().unwrap();
        let mut body: Value = serde_json::from_slice(&request.body).unwrap();
        body["output"] = json!({"title":"stale","summary":"stale","text":"stale"});
        request.body = serde_json::to_vec(&body).unwrap();
    }
    case.api.release();
    assert_eq!(case.api.0.lock().unwrap().visible.len(), 2);
    let client = GitHub::new(
        AppConfig {
            id: 123,
            slug: "factory".into(),
            pem: include_str!("../../mergify/tests/fixtures/throwaway-key.pem").into(),
            webhook_secret: "fixture".into(),
        },
        3,
        ClientOptions {
            api_url: format!("{}/", case.server.uri()).parse().unwrap(),
            mutation_interval: Duration::ZERO,
            max_retries: 0,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    let catchup = tl3_controller::ingress::CatchUp::new(BTreeMap::from([(3, client)])).unwrap();
    let mut store = Store::open(&case.settings).unwrap();
    // Recent cursor and empty pulls list exclude old closed PR updates.
    store
        .receive_catch_up(
            "github-catch-up",
            &[],
            &json!(chrono::Utc::now() - chrono::Duration::minutes(1)),
        )
        .unwrap();
    let inputs = catchup.pass(&mut store, &bootstrap()).unwrap();
    tl3_controller::ingress::drain(&mut controller).unwrap();
    for input in inputs {
        tl3_controller::ingress::apply(&mut controller, input).unwrap();
    }
    drain(&mut controller);
    case.assert_converged(2);
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "approval_dispatch_recovery"),
        "identity-checked evidence for both sends retires unresolved recovery"
    );
    assert_eq!(
        case.api.0.lock().unwrap().posts,
        2,
        "gap recovery never POSTs another approval"
    );
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

fn http_client(case: &Case) -> GitHub {
    GitHub::new(
        AppConfig {
            id: 123,
            slug: "factory".into(),
            pem: include_str!("../../mergify/tests/fixtures/throwaway-key.pem").into(),
            webhook_secret: "fixture".into(),
        },
        3,
        ClientOptions {
            api_url: format!("{}/", case.server.uri()).parse().unwrap(),
            request_timeout: Duration::from_secs(1),
            mutation_interval: Duration::ZERO,
            max_retries: 0,
            ..ClientOptions::default()
        },
    )
    .unwrap()
}

fn approval_adapter(case: &Case, epoch: i64) -> adapters::github::Adapter {
    adapters::github::Adapter::new(
        BTreeMap::from([(3, http_client(case))]),
        adapters::workers::FakeWriter,
        epoch,
        Policy::default(),
        &case.root.path().join("github"),
        Some(tl3_config::OperationsIssue {
            owner: "owner".into(),
            name: "operations".into(),
            installation_id: 3,
            number: 81,
        }),
    )
    .unwrap()
}

/// #766, state-table effect epochs: stale owners cannot repair or retire approval records.
#[test]
fn stale_owner_cannot_repair_approval_copies() {
    let case = Case::new("repair_epoch_fence");
    let delivery = case.delivery();
    let directory = case.root.path().join("github");
    std::fs::create_dir_all(&directory).unwrap();
    let attempt = |key: &str| {
        let external_id = format!("{key}:{}", delivery.head_sha.as_ref().unwrap());
        let encoded: String = external_id.bytes().map(|b| format!("{b:02x}")).collect();
        directory.join(format!("{encoded}.approval"))
    };
    // Both a pending attempt and a terminal legacy attempt must survive stale cleanup.
    std::fs::write(attempt(&delivery.key), b"\"Uncertain\"").unwrap();
    std::fs::write(attempt("retired-legacy-approval"), b"\"Observed\"").unwrap();
    let mut stale = approval_adapter(&case, 0);
    let mut current = approval_adapter(&case, 1);
    // The old owner already opened its legacy directory scan before migration completed.
    let marker = directory.join("approval-attempts.migrated");
    std::fs::write(&marker, b"completed by the current owner").unwrap();
    let local_records = || {
        std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "approval") || *path == marker)
            .map(|path| {
                (
                    path.file_name().unwrap().to_owned(),
                    std::fs::read(path).unwrap(),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let before = local_records();
    // Persist the new owner's fence without creating any check evidence.
    current.reconcile(&delivery).unwrap();
    case.corrupt_copies(2);
    let result = stale.repair_approval(&delivery);
    let retirement = stale.retire_delivery(&delivery);
    let after_delivery = local_records();
    let mut store = Store::open(&case.settings).unwrap();
    let migration = stale.retire(&mut store);
    let after_migration = local_records();
    assert_eq!(
        case.api.0.lock().unwrap().patches,
        0,
        "a stale owner must never PATCH approval copies"
    );
    assert!(
        result.is_err(),
        "repair must report the ownership fence refusal"
    );
    assert_eq!(
        after_delivery, before,
        "stale delivery retirement changed approval records"
    );
    assert_eq!(
        after_migration, before,
        "stale legacy retirement changed approval records or migration marker"
    );
    assert!(
        retirement.is_err(),
        "delivery retirement must refuse stale ownership"
    );
    assert!(
        migration.is_err(),
        "legacy retirement must refuse stale ownership"
    );
    current.repair_approval(&delivery).unwrap();
    case.assert_converged(2);
    current.retire_delivery(&delivery).unwrap();
    current.retire(&mut store).unwrap();
    assert_eq!(
        local_records(),
        BTreeMap::from([(marker.file_name().unwrap().to_owned(), b"1".to_vec())])
    );
}

/// #766/Q27: deployed-main approval rows must keep closed heads in actual gap catch-up.
#[test]
fn deployed_main_outstanding_post_survives_upgrade_stop_and_gap() {
    let case = Case::new("main_upgrade_closed_gap");
    let delivery = case.delivery();
    let mut store = Store::open(&case.settings).unwrap();
    let mut sql = fixture_connection(&case.settings);
    // Remove only #769's new schema. The effect payload and transition receipt below
    // are the deployed 46f30c8c format; memory tables do not participate in dispatch.
    sql.batch_execute(
        "DROP TRIGGER tl3_record_dispatch ON tl3_effects;
        DROP FUNCTION tl3_record_dispatch(); DROP FUNCTION tl3_approval_operation(jsonb);
        DROP TABLE tl3_dispatch_evidence; DROP TABLE tl3_dispatch_migration;
        DELETE FROM tl3_schema_migrations WHERE version=21",
    )
    .unwrap();
    let snapshot = store.load().unwrap();
    let mut sent = tl3_store::Commit::new(
        format!("delivery:{}:{}", case.key, snapshot.revision.get()),
        json!({"effect":case.key,"from":"Intended","to":"Sent"}),
        &snapshot,
    );
    sent.transitions.push(tl3_store::EffectTransition {
        key: case.key.clone(),
        from: EffectState::Intended,
        to: EffectState::Sent,
    });
    store.commit(&sent).unwrap();
    // Main's actual GET-then-POST client API writes no output. Keep its HTTP request
    // beyond the timeout, with no new approval attempt or recovery metadata.
    let error = case
        ._runtime
        .block_on(http_client(&case).create_check_run(
            "owner/sandbox",
            &tl3_github::NewCheckRun {
                name: "TL3 approval".into(),
                head_sha: delivery.head_sha.clone().unwrap(),
                external_id: format!("{}:{}", case.key, delivery.head_sha.as_ref().unwrap()),
                status: tl3_github::CheckStatus::Completed,
                conclusion: Some(tl3_github::CheckConclusion::Success),
            },
        ))
        .unwrap_err();
    assert!(matches!(error, tl3_github::Error::Transport));
    assert_eq!(case.api.0.lock().unwrap().posts, 1);
    assert!(case.api.0.lock().unwrap().held.is_some());
    assert!(
        !store
            .load()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table.starts_with("approval_"))
    );
    drop(store);
    // #785: auxiliary opens refuse an old schema. Only the claimed session may
    // install 0021; its migration must not replay history or create recovery rows.
    assert!(matches!(
        Store::open(&case.settings),
        Err(tl3_store::Error::SchemaVersion { .. })
    ));
    controller_restart::wait_for_owner_release(&mut sql);
    let mut owner = tl3_store::Ownership::claim(Store::connect(&case.settings).unwrap()).unwrap();
    owner.migrate().unwrap();
    assert!(
        !owner
            .store()
            .load()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "approval_dispatch_recovery")
    );
    drop(owner);
    // Stop arrives before the first new controller drive/backfill on its claimed session.
    let mut controller = case.open();
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "approval_dispatch_recovery")
    );
    controller
        .observe(
            "stop-at-deploy",
            tl3_controller::executor::Observation::Kernel(tl3_kernel::Event::Command {
                node: delivery.node(),
                id: 172,
                command: tl3_kernel::Command::Stop,
            }),
        )
        .unwrap();
    drain(&mut controller);
    assert!(matches!(
        controller.world().unwrap().nodes[0].phase,
        tl3_kernel::Phase::Closed(_)
    ));
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "allowances")
    );
    drop(controller);
    case.api.release();
    assert!(
        case.api.0.lock().unwrap().visible[0]
            .get("output")
            .is_none()
    );
    let mut controller = case.open();
    let catchup =
        tl3_controller::ingress::CatchUp::new(BTreeMap::from([(3, http_client(&case))])).unwrap();
    let mut store = Store::open(&case.settings).unwrap();
    store
        .receive_catch_up(
            "github-catch-up",
            &[],
            &json!(chrono::Utc::now() - chrono::Duration::minutes(1)),
        )
        .unwrap();
    let inputs = catchup.pass(&mut store, &bootstrap()).unwrap();
    tl3_controller::ingress::drain(&mut controller).unwrap();
    for input in inputs {
        tl3_controller::ingress::apply(&mut controller, input).unwrap();
    }
    drain(&mut controller);
    case.assert_converged(1);
    assert_eq!(
        case.api.0.lock().unwrap().visible[0]["output"]["title"],
        "TL3 approval",
        "the output-free deployed-main POST must be repaired on the closed head"
    );
    assert_eq!(
        case.api.0.lock().unwrap().posts,
        1,
        "gap recovery must never POST"
    );
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "approval_dispatch_recovery")
    );
}
