use tl3_controller::executor::Output as Out;
#[path = "github/notice_incidents.rs"]
mod notice_incidents;
#[path = "github/steering.rs"]
mod steering;
#[path = "../../store/tests/support/mod.rs"]
mod support;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use rsa::{
    RsaPrivateKey,
    pkcs8::{EncodePrivateKey, LineEnding},
};
use serde_json::{Value, json};
use sha2::Sha256;
use std::{
    collections::BTreeMap,
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, LazyLock, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use tl3_controller::{
    Bootstrap, Boundary, Controller, LabelledIssue, Repository, adapters,
    adapters::workers::writer::{
        FakeWriter, MessageKind, WriteRequest, Writer, WriterReply, checked_draft,
    },
    cli::Input,
    executor::{Delivery, Executor, Observation, Reconciliation, Upstream},
    ingress::{self, CatchUp, GitHubDelivery, GitHubTranslator, Source, Sources, Webhooks},
};
use tl3_github::{AppConfig, ClientOptions, GitHub, message::Policy};
use tl3_kernel::{NodeId, Notice, Outcome, Phase};
use tl3_store::Store;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

static PEM: LazyLock<String> = LazyLock::new(|| {
    RsaPrivateKey::new(&mut OsRng, 2048)
        .unwrap()
        .to_pkcs8_pem(LineEnding::LF)
        .unwrap()
        .to_string()
});
fn bootstrap() -> Bootstrap {
    Bootstrap {
        operations: None,
        repos: vec![Repository {
            owner: "owner".into(),
            name: "sandbox".into(),
            installation: 3,
        }],
        rob: 42,
        slots: 1,
        pool: 1,
    }
}
fn fixture(comment: bool) -> Value {
    serde_json::from_str(if comment {
        include_str!("../../github/tests/fixtures/issue-comment-created.json")
    } else {
        include_str!("../../github/tests/fixtures/issues-labeled.json")
    })
    .unwrap()
}
fn labelled() -> LabelledIssue {
    LabelledIssue {
        delivery: "fixture-label".into(),
        owner: "owner".into(),
        repo: "sandbox".into(),
        installation: 3,
        number: 12,
        title: "Add a blue button".into(),
        actor: 42,
        label: "tl3".into(),
    }
}
fn client(uri: &str) -> GitHub {
    client_with_pem(uri, PEM.clone())
}
fn client_with_pem(uri: &str, pem: String) -> GitHub {
    GitHub::new(
        AppConfig {
            id: 123,
            slug: "factory".into(),
            pem,
            webhook_secret: "test-secret".into(),
        },
        3,
        ClientOptions {
            api_url: format!("{uri}/").parse().unwrap(),
            max_retries: 0,
            mutation_interval: Duration::ZERO,
            max_wait: Duration::ZERO,
            ..ClientOptions::default()
        },
    )
    .unwrap()
}
#[derive(Default)]
struct ApiState {
    comments: Vec<Value>,
    posts: usize,
    deletes: usize,
    closed: bool,
    gap: bool,
    catch_up_passes: usize,
    fail_reads: bool,
    empty_body: bool,
    catch_up_evidence: bool,
    redeliveries: Vec<u64>,
    new_delivery: bool,
    linked: Vec<Value>,
    lose_issue_reply: bool,
    forbidden_question: bool,
    forbidden_filing: bool,
}
#[derive(Clone)]
struct Api(Arc<Mutex<ApiState>>);
impl Respond for Api {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.0.lock().unwrap();
        let original_path = request.url.path();
        let mapped_path = original_path
            .replace(
                "/repos/owner/tl3/issues/17",
                "/repos/owner/sandbox/issues/12",
            )
            .replace(
                "/repos/owner/tl3/issues/comments/",
                "/repos/owner/sandbox/issues/comments/",
            );
        let path = mapped_path.as_str();
        let method = request.method.as_str();
        if path == "/app/installations/3/access_tokens" {
            return ResponseTemplate::new(201).set_body_json(
                json!({"token": "disposable-test-token", "expires_at": "2099-01-01T00:00:00Z"}),
            );
        }
        if state.fail_reads && method == "GET" {
            return ResponseTemplate::new(503);
        }
        match (method, path) {
            ("GET", "/app/hook/deliveries") => {
                let jwt = request
                    .headers
                    .get("authorization")
                    .unwrap()
                    .to_str()
                    .unwrap();
                assert_eq!(
                    jwt.split('.').count(),
                    3,
                    "delivery log requires an app JWT"
                );
                if request.url.query_pairs().any(|(key, _)| key == "cursor") {
                    ResponseTemplate::new(200).set_body_json(
                        json!([{"id":18,"status_code":503,"delivered_at":"2026-10-30T00:00:00Z"}]),
                    )
                } else {
                    let mut deliveries = vec![
                        json!({"id":20,"status_code":500,"delivered_at":"2026-10-30T00:00:02Z"}),
                        json!({"id":19,"status_code":200,"delivered_at":"2026-10-30T00:00:01Z"}),
                    ];
                    if state.new_delivery {
                        deliveries.insert(0, json!({"id":22,"status_code":500,"delivered_at":(Utc::now() + chrono::Duration::seconds(1)).to_rfc3339()}));
                    }
                    ResponseTemplate::new(200)
                        .insert_header(
                            "link",
                            format!(
                                "<{}app/hook/deliveries?cursor=older>; rel=\"next\"",
                                request.url.join("/").unwrap()
                            ),
                        )
                        .set_body_json(deliveries)
                }
            }
            ("POST", path)
                if path.starts_with("/app/hook/deliveries/") && path.ends_with("/attempts") =>
            {
                let id = path.split('/').nth(4).unwrap().parse().unwrap();
                state.redeliveries.push(id);
                ResponseTemplate::new(202)
            }

            ("GET", path)
                if path.starts_with("/repos/owner/sandbox/issues/")
                    && path.ends_with("/comments") =>
            {
                ResponseTemplate::new(200).set_body_json(json!(&state.comments))
            }
            ("POST", path)
                if path.starts_with("/repos/owner/sandbox/issues/")
                    && path.ends_with("/comments") =>
            {
                if state.forbidden_question
                    && original_path.contains("/sandbox/")
                    && String::from_utf8_lossy(&request.body).contains("recommend option")
                {
                    return ResponseTemplate::new(403);
                }
                state.posts += 1;
                let mut comment = fixture(true)["comment"].clone();
                comment["id"] = json!(100 + state.posts as u64);
                comment["user"] = json!({"id": 99, "login": "factory[bot]"});
                comment["body"] =
                    serde_json::from_slice::<Value>(&request.body).unwrap()["body"].clone();
                state.comments.push(comment.clone());
                ResponseTemplate::new(201).set_body_json(comment)
            }
            ("DELETE", path) if path.starts_with("/repos/owner/sandbox/issues/comments/") => {
                let id: u64 = path.rsplit('/').next().unwrap().parse().unwrap();
                state.comments.retain(|comment| comment["id"] != id);
                state.deletes += 1;
                ResponseTemplate::new(204)
            }
            ("PATCH", path) if path.starts_with("/repos/owner/sandbox/issues/") => {
                state.closed = true;
                let mut issue = fixture(false)["issue"].clone();
                issue["state"] = json!("closed");
                ResponseTemplate::new(200).set_body_json(issue)
            }
            ("GET", "/repos/owner/sandbox/issues/12") => {
                let mut issue = fixture(false)["issue"].clone();
                if state.empty_body {
                    issue["body"] = Value::Null;
                }
                if state.closed {
                    issue["state"] = json!("closed");
                }
                ResponseTemplate::new(200).set_body_json(issue)
            }
            (
                "GET",
                "/repos/owner/tl3/issues"
                | "/repos/owner/tl3/issues/comments"
                | "/repos/owner/tl3/issues/events"
                | "/repos/owner/tl3/pulls",
            ) => ResponseTemplate::new(200).set_body_json(json!([])),
            ("POST", "/repos/owner/sandbox/issues") => {
                if state.forbidden_filing {
                    return ResponseTemplate::new(403);
                }
                let payload: Value = serde_json::from_slice(&request.body).unwrap();
                let mut issue = fixture(false)["issue"].clone();
                issue["number"] = json!(1000 + state.linked.len());
                issue["title"] = payload["title"].clone();
                issue["body"] = payload["body"].clone();
                issue["labels"] = json!([{"name": "tl3"}]);
                issue["user"] = json!({"id": 99, "login": "factory[bot]"});
                assert_eq!(payload["labels"], json!(["tl3"]));
                state.linked.push(issue.clone());
                if state.lose_issue_reply {
                    state.lose_issue_reply = false;
                    ResponseTemplate::new(500)
                } else {
                    ResponseTemplate::new(201).set_body_json(issue)
                }
            }
            ("GET", "/repos/owner/sandbox/issues") => {
                state.catch_up_passes += 1;
                let mut issues = state.linked.clone();
                issues.push(fixture(false)["issue"].clone());
                ResponseTemplate::new(200).set_body_json(issues)
            }
            ("GET", "/repos/owner/sandbox/issues/comments") => ResponseTemplate::new(200)
                .set_body_json(if state.gap {
                    {
                        let mut comment = fixture(true)["comment"].clone();
                        comment["created_at"] = json!(Utc::now());
                        comment["updated_at"] = comment["created_at"].clone();
                        if state.catch_up_evidence {
                            comment["issue_url"] =
                                json!("https://api.github.com/repos/OWNER/SANDBOX/issues/12");
                            let mut pr_comment = comment.clone();
                            pr_comment["id"] = json!(901);
                            pr_comment["issue_url"] =
                                json!("https://api.github.com/repos/OWNER/SANDBOX/issues/25");
                            json!([comment, pr_comment])
                        } else {
                            json!([comment])
                        }
                    }
                } else {
                    json!([])
                }),
            ("GET", "/repos/owner/sandbox/issues/events") => {
                let events = if state.gap {
                    json!([{"id": 77, "event": "labeled", "actor": {"id": 42, "login": "owner"}, "label": {"name": "tl3"}, "created_at": Utc::now(), "issue": {"number": 12}}])
                } else {
                    json!([])
                };
                ResponseTemplate::new(200).set_body_json(events)
            }
            ("GET", "/repos/owner/sandbox/pulls") => {
                let pulls = if state.catch_up_evidence {
                    let mut pull = event_fixture("pull_request")["pull_request"].clone();
                    pull["updated_at"] = json!(Utc::now());
                    json!([pull])
                } else {
                    json!([])
                };
                ResponseTemplate::new(200).set_body_json(pulls)
            }
            ("GET", path) if path.ends_with("/check-runs") => ResponseTemplate::new(200)
                .set_body_json(if state.catch_up_evidence {
                    json!({"check_runs": [event_fixture("check_run")["check_run"]]})
                } else {
                    json!({"check_runs": []})
                }),
            ("GET", path) if path.ends_with("/check-suites") => ResponseTemplate::new(200)
                .set_body_json(if state.catch_up_evidence {
                    json!({"check_suites": [event_fixture("check_suite")["check_suite"]]})
                } else {
                    json!({"check_suites": []})
                }),
            ("GET", path) if path.starts_with("/repos/owner/sandbox/issues/1") => {
                let number: u64 = path.rsplit('/').next().unwrap().parse().unwrap();
                ResponseTemplate::new(200)
                    .set_body_json(state.linked.iter().find(|v| v["number"] == number).unwrap())
            }
            _ => panic!("unexpected mock API call {method} {path}"),
        }
    }
}
struct Server {
    runtime: tokio::runtime::Runtime,
    server: MockServer,
    state: Arc<Mutex<ApiState>>,
}
impl Server {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let state = Arc::new(Mutex::new(ApiState::default()));
        let server = runtime.block_on(async {
            let server = MockServer::start().await;
            Mock::given(|_: &Request| true)
                .respond_with(Api(state.clone()))
                .mount(&server)
                .await;
            server
        });
        Self {
            runtime,
            server,
            state,
        }
    }
    fn uri(&self) -> String {
        self.server.uri()
    }
    fn requests(&self) -> Vec<Request> {
        self.runtime
            .block_on(self.server.received_requests())
            .unwrap()
    }
}
fn adapter(uri: &str, root: &Path) -> adapters::github::Adapter {
    adapter_with_client(client(uri), root)
}
fn adapter_with_client(client: GitHub, root: &Path) -> adapters::github::Adapter {
    adapters::github::Adapter::new(
        BTreeMap::from([(3, client)]),
        FakeWriter,
        0,
        Policy {
            known_paths: ["workers/src".into()].into(),
            ..Policy::default()
        },
        root,
        Some(tl3_config::OperationsIssue {
            owner: "owner".into(),
            name: "tl3".into(),
            installation_id: 3,
            number: 17,
        }),
    )
    .unwrap()
}
fn controller(settings: &tl3_config::Store, root: &Path, uri: &str) -> Controller {
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(Upstream::GitHub, adapter(uri, root));
    Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap()
}
fn drain(controller: &mut Controller) {
    ingress::drain(controller).unwrap();
    for _ in 0..200 {
        if !controller.drive_once().unwrap() {
            return;
        }
    }
    panic!("controller failed to settle");
}
fn post(address: SocketAddr, id: &str, comment: bool, body: Value, signature_valid: bool) -> u16 {
    post_event(
        address,
        id,
        if comment { "issue_comment" } else { "issues" },
        body,
        signature_valid,
    )
}
fn post_event(
    address: SocketAddr,
    id: &str,
    event: &str,
    body: Value,
    signature_valid: bool,
) -> u16 {
    let body = serde_json::to_vec(&body).unwrap();
    let mut mac = Hmac::<Sha256>::new_from_slice(b"test-secret").unwrap();
    mac.update(&body);
    let signature = if signature_valid {
        hex::encode(mac.finalize().into_bytes())
    } else {
        "0".repeat(64)
    };
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(stream, "POST /github/events HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nX-GitHub-Event: {}\r\nX-GitHub-Delivery: {id}\r\nX-Hub-Signature-256: sha256={signature}\r\n\r\n", body.len(), event).unwrap();
    stream.write_all(&body).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response.split_whitespace().nth(1).unwrap().parse().unwrap()
}
fn main() {
    let args: Vec<_> = env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--crash") {
        crash_child(&args);
        return;
    }
    if args
        .get(1)
        .is_some_and(|arg| arg == "--merge-pending-child")
    {
        steering::merge_pending_child(&args);
        return;
    }
    if args.get(1).is_some_and(|arg| arg == "--notice-retry-child") {
        notice_incidents::child(&args);
        return;
    }
    let mut database = support::Database::start();
    if args.get(1).is_some_and(|arg| arg == "--notice-incidents") {
        notice_incidents::run(&mut database);
        database.stop();
        return;
    }
    notice_incidents::run(&mut database);
    operations_and_delivery_log(&database.create("operations"));
    println!("test operations_messages_and_missed_delivery_recovery ... ok");
    if let Some(scenario) = args.get(1).and_then(|arg| arg.strip_prefix("--followup=")) {
        match scenario {
            "quote" => steering::dropped_change_quote(),
            "second" => steering::second_replacement(&database.create("second_replacement")),
            "merge" => steering::merge_pending(&database.create("merge_pending"), "none"),
            _ => panic!("unknown followup scenario"),
        }
        database.stop();
        return;
    }
    receiver_is_durable(&database.create("receiver"));
    println!("test receiver_is_durable ... ok");
    gap_catch_up(&database.create("gap"));
    println!("test gap_catch_up ... ok");
    post_crash_recovers(&database.create("post_crash"));
    println!("test post_crash_recovers ... ok");
    message_reconciliation();
    println!("test message_reconciliation ... ok");
    writer_checks();
    println!("test writer_checks ... ok");
    receiver_limits(&database.create("receiver_limits"));
    println!("test receiver_limits ... ok");
    source_failures(&database.create("source_failures"));
    println!("test source_failures ... ok");
    delivery_reconnects(&database.create("reconnect"), &mut database);
    println!("test delivery_reconnects ... ok");
    parsed_deliveries_and_translators(&database.create("parsed"));
    println!("test parsed_deliveries_and_translators ... ok");
    catch_up_preserves_evidence(&database.create("catch_up_evidence"));
    println!("test catch_up_preserves_evidence ... ok");
    daemon_with_workers(&database.create("daemon_workers"));
    println!("test daemon_webhooks_worker_replies_and_source_failure ... ok");
    steering::linked_reconciliation();
    steering::commands(&database.create("steering_commands"));
    steering::answers(&database.create("steering_answers"));
    steering::replacement(&database.create("steering_replacement"));
    steering::correction(&database.create("steering_correction"));
    steering::queued_pause(&database.create("steering_queue"));
    steering::answer_recovery(&database.create("steering_answer_recovery"));
    steering::classifier_failures(&database.create("classifier_failures"));
    steering::concurrent_requests(&database.create("concurrent_requests"));
    steering::guidance(&database.create("steering_guidance"));
    steering::credit(&database.create("steering_credit"));
    steering::answer_replacement(&database.create("answer_replacement"));
    steering::writer_rejection();
    for race in ["label", "merge", "crash"] {
        steering::replacement_race(
            &database.create(&format!("replacement_window_{race}")),
            race,
        );
    }
    steering::unanswered_clarification_closes(&database.create("unanswered_clarification"));
    steering::dropped_change_quote();
    steering::second_replacement(&database.create("second_replacement"));
    for point in [
        "none",
        "EventWritten",
        "RowsWritten",
        "EffectsWritten",
        "TransitionsWritten",
        "CreditWritten",
        "BeforeCommit",
        "AfterCommit",
        "BeforeDispatch",
        "AfterDispatch",
        "BeforeReconcile",
        "AfterReconcile",
    ] {
        steering::merge_pending(&database.create(&format!("pending_{point}")), point);
    }
    database.stop();
    println!("GitHub integration scenarios passed; no skips; no live GitHub calls");
}
fn receiver_is_durable(settings: &tl3_config::Store) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let source = Webhooks::bind(
        "127.0.0.1:0".parse().unwrap(),
        Store::open(settings).unwrap(),
        bootstrap(),
        b"test-secret".to_vec(),
        None,
    )
    .unwrap();
    let address = source.address();
    let control = source.control();
    let (sender, receiver) = mpsc::channel();
    let source_thread = thread::spawn(move || source.run(sender));
    assert_eq!(post(address, "invalid", false, fixture(false), false), 400);
    assert!(
        Store::open(settings)
            .unwrap()
            .pending_ingress()
            .unwrap()
            .is_empty()
    );
    let mut other = fixture(false);
    other["sender"]["id"] = json!(7);
    other["issue"]["number"] = json!(13);
    assert_eq!(post(address, "other", false, other, true), 202);
    assert_eq!(post(address, "label", false, fixture(false), true), 202);
    assert_eq!(post(address, "label", false, fixture(false), true), 202);
    assert_eq!(post(address, "comment", true, fixture(true), true), 202);
    let mut changed = fixture(false);
    changed["installation"]["id"] = json!(4);
    changed["issue"]["number"] = json!(14);
    assert_eq!(post(address, "wrong-install", false, changed, true), 202);
    // Every 2xx above has a persisted delivery even though no controller exists yet.
    assert_eq!(
        Store::open(settings)
            .unwrap()
            .pending_ingress()
            .unwrap()
            .len(),
        4
    );
    control.stop();
    source_thread.join().unwrap().unwrap();
    drop(receiver);
    let mut controller = controller(settings, root.path(), &api.uri());
    drain(&mut controller);
    assert_eq!(controller.world().unwrap().nodes.len(), 1);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)
    );
    assert_eq!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|row| row.table == "github_comments")
            .count(),
        0
    );
    assert!(api.state.lock().unwrap().closed);
    assert_eq!(api.state.lock().unwrap().posts, 2);
    assert!(
        Store::open(settings)
            .unwrap()
            .pending_ingress()
            .unwrap()
            .is_empty()
    );
}
fn gap_catch_up(settings: &tl3_config::Store) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let source = Webhooks::bind(
        "127.0.0.1:0".parse().unwrap(),
        Store::open(settings).unwrap(),
        bootstrap(),
        b"test-secret".to_vec(),
        Some(CatchUp::new(BTreeMap::from([(3, client(&api.uri()))])).unwrap()),
    )
    .unwrap();
    let address = source.address();
    let control = source.control();
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || source.run(sender));
    // Synchronize on the startup pass with a real webhook, without a polling timer.
    let mut other = fixture(false);
    other["sender"]["id"] = json!(7);
    assert_eq!(post(address, "wake-start", false, other, true), 202);
    let _ = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    api.state.lock().unwrap().gap = true;
    control.gap();
    let _ = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    control.stop();
    handle.join().unwrap().unwrap();
    let mut store = Store::open(settings).unwrap();
    assert!(store.ingress_cursor("github-catch-up").unwrap().is_some());
    let pending = store.pending_ingress().unwrap();
    assert!(
        store
            .receive_catch_up(
                "rollback",
                &[("one".into(), b"[]".to_vec()), ("".into(), b"[]".to_vec())],
                &json!("must not advance")
            )
            .is_err()
    );
    assert!(store.ingress_cursor("rollback").unwrap().is_none());
    assert_eq!(store.pending_ingress().unwrap(), pending);
    let mut controller = controller(settings, root.path(), &api.uri());
    drain(&mut controller);
    assert_eq!(controller.world().unwrap().nodes.len(), 1);
    assert_eq!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|row| row.table == "github_comments")
            .count(),
        0
    );
    let catch_up = CatchUp::new(BTreeMap::from([(3, client(&api.uri()))])).unwrap();
    catch_up.pass(&mut store, &bootstrap()).unwrap();
    drain(&mut controller);
    assert_eq!(controller.world().unwrap().nodes.len(), 1);
    assert_eq!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|row| row.table == "github_comments")
            .count(),
        0
    );
    let before = store.ingress_cursor("github-catch-up").unwrap();
    api.state.lock().unwrap().fail_reads = true;
    assert!(catch_up.pass(&mut store, &bootstrap()).is_err());
    assert_eq!(store.ingress_cursor("github-catch-up").unwrap(), before);
    assert_eq!(api.state.lock().unwrap().catch_up_passes, 3);
    assert!(
        api.requests()
            .iter()
            .all(|request| request.url.host_str() == Some("localhost")
                || request.url.host_str() == Some("127.0.0.1"))
    );
}
fn post_crash_recovers(settings: &tl3_config::Store) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut child = Command::new(env::current_exe().unwrap())
        .arg("--crash")
        .arg(settings.port.to_string())
        .arg(&settings.database)
        .arg(settings.password_file.as_ref().unwrap())
        .arg(root.path())
        .arg(api.uri())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "POSTED");
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success());
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
    }
    assert_eq!(api.state.lock().unwrap().posts, 1);
    let mut restored = controller(settings, root.path(), &api.uri());
    drain(&mut restored);
    assert_eq!(
        restored.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)
    );
    let state = api.state.lock().unwrap();
    assert_eq!(state.posts, 2, "one goals note and one closing summary");
    assert!(state.closed);
    assert_eq!(state.comments.len(), 2);
    assert!(state.comments.iter().all(|comment| {
        comment["body"]
            .as_str()
            .unwrap()
            .starts_with("<!-- tl3:effect:")
    }));
}
fn crash_child(args: &[String]) {
    let settings = tl3_config::Store {
        host: "127.0.0.1".into(),
        port: args[2].parse().unwrap(),
        database: args[3].clone(),
        user: "postgres".into(),
        password_file: Some(args[4].clone().into()),
        connect_timeout_secs: 10,
    };
    let mut controller = controller(&settings, Path::new(&args[5]), &args[6]);
    controller.intake(labelled()).unwrap();
    loop {
        assert!(
            controller
                .drive_once_with(|boundary, key| {
                    if boundary == Boundary::AfterDispatch && key.starts_with("notice:") {
                        println!("POSTED");
                        std::io::stdout().flush().unwrap();
                        loop {
                            thread::park();
                        }
                    }
                })
                .unwrap()
        );
    }
}
fn delivery(notice: Notice) -> Delivery {
    Delivery {
        key: "notice:0:1".into(),
        epoch: 0,
        output: Out::Notice {
            node: NodeId(0),
            notice,
        },
        generation: 0,
        head_sha: None,
        repository: bootstrap().repos[0].clone(),
        issue: 12,
        pull_request: None,
        paths: vec![],
        attempt: 1,
        round: 0,
        replanned: false,
        question: None,
        question_context: None,
        steering: None,
        request_text: None,
        checks: None,
        job_generation: None,
        marker_key: None,
        note: None,
    }
}
fn message_reconciliation() {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut adapter = adapter(&api.uri(), root.path());
    let note = delivery(Notice::MeasuresPosted);
    api.state.lock().unwrap().empty_body = true;
    assert!(matches!(
        adapter.reconcile(&note).unwrap(),
        Reconciliation::Absent
    ));
    adapter.execute(&note).unwrap();
    adapter.execute(&note).unwrap();
    assert_eq!(api.state.lock().unwrap().posts, 1);
    {
        let mut state = api.state.lock().unwrap();
        let mut duplicate = state.comments[0].clone();
        duplicate["id"] = json!(500);
        state.comments.push(duplicate.clone());
        duplicate["id"] = json!(600);
        duplicate["user"]["login"] = json!("someone-else");
        state.comments.push(duplicate);
    }
    assert!(matches!(
        adapter.reconcile(&note).unwrap(),
        Reconciliation::Found(_)
    ));
    assert_eq!(api.state.lock().unwrap().deletes, 1);
    assert_eq!(api.state.lock().unwrap().comments.len(), 2);
    api.state.lock().unwrap().fail_reads = true;
    assert!(matches!(
        adapter.reconcile(&note).unwrap(),
        Reconciliation::Pending
    ));
    api.state.lock().unwrap().fail_reads = false;
    let mut newer = note.clone();
    newer.epoch = 1;
    let mut current = adapters::github::Adapter::new(
        BTreeMap::from([(3, client(&api.uri()))]),
        FakeWriter,
        1,
        Policy::default(),
        root.path(),
        Some(tl3_config::OperationsIssue {
            owner: "owner".into(),
            name: "tl3".into(),
            installation_id: 3,
            number: 17,
        }),
    )
    .unwrap();
    current.execute(&newer).unwrap();
    assert!(matches!(
        current.reconcile(&note).unwrap(),
        Reconciliation::Found(_)
    ));
    assert!(adapter.execute(&note).is_err());
    let mut restarted = adapters::github::Adapter::new(
        BTreeMap::from([(3, client(&api.uri()))]),
        FakeWriter,
        0,
        Policy::default(),
        root.path(),
        Some(tl3_config::OperationsIssue {
            owner: "owner".into(),
            name: "tl3".into(),
            installation_id: 3,
            number: 17,
        }),
    )
    .unwrap();
    assert!(restarted.execute(&note).is_err());
    adapter = current;
    let mut summary = delivery(Notice::Summary(Outcome::Replaced));
    summary.epoch = 1;
    summary.key = "notice:0:2".into();
    adapter.execute(&summary).unwrap();
    assert!(!api.state.lock().unwrap().closed);
    let mut summary = delivery(Notice::Summary(Outcome::Stopped));
    summary.epoch = 1;
    summary.key = "notice:0:3".into();
    adapter.execute(&summary).unwrap();
    assert!(api.state.lock().unwrap().closed);
    api.state.lock().unwrap().closed = false;
    assert!(matches!(
        adapter.reconcile(&summary).unwrap(),
        Reconciliation::Found(_)
    ));
    assert!(api.state.lock().unwrap().closed);
    let mut pr = newer;
    pr.key = "effect:0:1".into();
    pr.output = Out::Effect {
        node: NodeId(0),
        seq: 1,
        kind: tl3_kernel::EffectKind::OpenPr,
        head: None,
    };
    adapter.execute(&pr).unwrap();
    assert!(matches!(
        adapter.reconcile(&pr).unwrap(),
        Reconciliation::Found(_)
    ));
    let before = api.state.lock().unwrap().posts;
    let mut incident = delivery(Notice::Incident { needs_rob: false });
    incident.epoch = 1;
    incident.key = "notice:0:4".into();
    assert!(adapter.execute(&incident).is_err());
    assert_eq!(api.state.lock().unwrap().posts, before);
}
struct ScriptWriter {
    calls: Arc<Mutex<Vec<WriteRequest>>>,
    failures: usize,
}
impl Writer for ScriptWriter {
    fn draft(&mut self, _: &str, input: &WriteRequest) -> tl3_controller::Result<WriterReply> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(input.clone());
        let mut reply = FakeWriter.draft("fake", input)?;
        if calls.len() <= self.failures {
            match &mut reply {
                WriterReply::Question(question) => {
                    question.obstacle = "The leaf needs attention.".into()
                }
                WriterReply::Text(text) => {
                    text.text = "About \"Maybe do that\", did the leaf fail?".into()
                }
            }
        }
        Ok(reply)
    }
}
fn writer_checks() {
    let input = WriteRequest {
        kind: MessageKind::Question,
        facts: json!({
        "issue_title": "Add a blue button", "request": "Add a blue button", "obstacle": "Your artwork needs permission.",
        "options": [{"action": "Choose another image", "outcome": "I can finish using a free image."}, {"action": "Grant permission", "outcome": "I can finish with the original image."}],
        "recommended_option": 0, "recommendation_reason": "The free image avoids a delay.", "how_to_answer": "Reply with option one or two."}),
        violations: vec![],
    };
    let calls = Arc::new(Mutex::new(vec![]));
    let mut writer = ScriptWriter {
        calls: calls.clone(),
        failures: 1,
    };
    assert!(checked_draft(&mut writer, "effect", input.clone(), &Policy::default()).is_ok());
    assert_eq!(calls.lock().unwrap().len(), 2);
    let feedback = calls.lock().unwrap()[1].violations.clone();
    assert!(feedback[0].contains("leaf") && feedback[0].contains("obstacle"));
    assert!(!feedback[0].contains("InternalVocabulary"));
    calls.lock().unwrap().clear();
    writer.failures = usize::MAX;
    assert!(checked_draft(&mut writer, "effect", input.clone(), &Policy::default()).is_err());
    assert_eq!(calls.lock().unwrap().len(), 3);
    calls.lock().unwrap().clear();
    let mut invalid = input;
    invalid.facts["recommended_option"] = json!(2);
    assert!(checked_draft(&mut writer, "effect", invalid, &Policy::default()).is_err());
    assert!(calls.lock().unwrap().is_empty());
    let mut draft = delivery(Notice::Ask { question: 9 });
    draft.question = Some(tl3_kernel::Question {
        id: 9,
        kind: tl3_kernel::QKind::NeedsSplitting,
        cont: tl3_kernel::Cont::Unplanned,
        cost: 0,
        clarified: false,
        post_failed: false,
    });
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    adapter(&api.uri(), root.path()).execute(&draft).unwrap();
    let comment = api.state.lock().unwrap().comments[0]["body"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(comment.contains("I recommend option 1"));
    assert!(comment.contains("Change the request"));
    calls.lock().unwrap().clear();
    let rejected = ScriptWriter {
        calls: calls.clone(),
        failures: usize::MAX,
    };
    draft.key = "notice:rejected:0".into();
    let mut rejecting = adapters::github::Adapter::new(
        BTreeMap::from([(3, client(&api.uri()))]),
        rejected,
        0,
        Policy::default(),
        root.path(),
        Some(tl3_config::OperationsIssue {
            owner: "owner".into(),
            name: "tl3".into(),
            installation_id: 3,
            number: 17,
        }),
    )
    .unwrap();
    let posted = api.state.lock().unwrap().posts;
    for notice in [
        Notice::Ask { question: 9 },
        Notice::Summary(Outcome::Stopped),
    ] {
        calls.lock().unwrap().clear();
        draft.output = Out::Notice {
            node: NodeId(0),
            notice,
        };
        assert_eq!(
            rejecting.execute(&draft).unwrap().ack,
            tl3_kernel::Ack::Permanent
        );
        assert_eq!(calls.lock().unwrap().len(), 3);
        let state = api.state.lock().unwrap();
        assert_eq!(state.posts, posted);
        assert!(!state.closed, "a rejected summary must not close the issue");
    }
}

fn raw_http(address: SocketAddr, request: &[u8]) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request).unwrap();
    response(&mut stream)
}
fn response(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}
fn start_receiver(
    settings: &tl3_config::Store,
) -> (
    SocketAddr,
    ingress::WebhookControl,
    thread::JoinHandle<tl3_controller::Result<()>>,
) {
    let source = Webhooks::bind(
        "127.0.0.1:0".parse().unwrap(),
        Store::open(settings).unwrap(),
        bootstrap(),
        b"test-secret".to_vec(),
        None,
    )
    .unwrap();
    let address = source.address();
    let control = source.control();
    let (sender, _receiver) = mpsc::channel();
    let handle = thread::spawn(move || source.run(sender));
    (address, control, handle)
}
fn receiver_limits(settings: &tl3_config::Store) {
    for address in ["0.0.0.0:0", "[::]:0", "192.168.1.2:8081"] {
        assert!(
            Webhooks::bind(
                address.parse().unwrap(),
                Store::open(settings).unwrap(),
                bootstrap(),
                b"test-secret".to_vec(),
                None
            )
            .is_err()
        );
    }
    let (address, control, handle) = start_receiver(settings);
    let mut stalled = TcpStream::connect(address).unwrap();
    stalled
        .write_all(b"POST /github/events HTTP/1.1\r\nContent-Length: 100\r\n\r\nx")
        .unwrap();
    assert_eq!(
        post(address, "during-stalled-body", false, fixture(false), true),
        202
    );
    assert!(response(&mut stalled).starts_with("HTTP/1.1 408"));
    assert_eq!(
        post(address, "after-stalled-body", false, fixture(false), true),
        202
    );

    let mut oversized = b"POST /github/events HTTP/1.1\r\nX-Large: ".to_vec();
    oversized.resize(8 * 1024, b'x');
    assert!(raw_http(address, &oversized).starts_with("HTTP/1.1 431"));
    assert_eq!(
        post(address, "after-large-header", false, fixture(false), true),
        202
    );
    assert!(
        raw_http(
            address,
            b"POST /github/events HTTP/1.1\r\nContent-Length: 26214401\r\n\r\n"
        )
        .starts_with("HTTP/1.1 413")
    );
    assert_eq!(
        post(address, "after-large-body", false, fixture(false), true),
        202
    );

    let blocked: Vec<_> = (0..8)
        .map(|_| TcpStream::connect(address).unwrap())
        .collect();
    let mut refused = TcpStream::connect(address).unwrap();
    refused
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    match refused.read(&mut [0]) {
        Ok(0) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        result => panic!("ninth connection was not refused: {result:?}"),
    }
    for mut blocked in blocked {
        blocked.shutdown(Shutdown::Write).unwrap();
        assert!(response(&mut blocked).starts_with("HTTP/1.1 400"));
    }
    assert_eq!(post(address, "after-cap", false, fixture(false), true), 202);
    for method in ["GET", "HEAD", "PUT", "DELETE", "OPTIONS"] {
        let reply = raw_http(
            address,
            format!("{method} /github/events HTTP/1.1\r\nContent-Length: 0\r\n\r\n").as_bytes(),
        );
        assert!(reply.starts_with("HTTP/1.1 405"), "{reply}");
        assert!(reply.contains("Allow: POST\r\n"), "{reply}");
    }
    for (headers, status) in [
        ("Transfer-Encoding: chunked", 400),
        ("Content-Length: 1\r\nContent-Length: 1", 400),
        ("Content-Length: -1", 400),
        ("Content-Length: +5", 400),
        ("Content-Length: 05", 400),
        ("Content-Length: 00", 400),
        ("Content-Length: \u{a0}5", 400),
        ("Content-Length: 999999999999999999999999999999999999", 413),
        ("Transfer-Encoding: chunked\r\nContent-Length: 5", 400),
        ("Content-Length: 5\r\nTransfer-Encoding: chunked", 400),
        (
            "Transfer-Encoding: chunked\r\nTransfer-Encoding: chunked",
            400,
        ),
        ("", 411),
    ] {
        assert!(
            raw_http(
                address,
                format!("POST /github/events HTTP/1.1\r\n{headers}\r\n\r\n").as_bytes()
            )
            .starts_with(&format!("HTTP/1.1 {status}"))
        );
    }
    assert!(
        raw_http(
            address,
            b"POST /webhooks/github HTTP/1.1\r\nContent-Length: 0\r\n\r\n"
        )
        .starts_with("HTTP/1.1 404")
    );
    assert_eq!(
        post(
            address,
            "after-invalid-framing",
            false,
            fixture(false),
            true
        ),
        202
    );
    let mut large = fixture(false);
    large["padding"] = json!("x".repeat(3 * 1024 * 1024));
    assert_eq!(post(address, "multi-read", false, large, true), 202);
    let body = br#"{"zen":"first request only"}"#;
    let mut mac = Hmac::<Sha256>::new_from_slice(b"test-secret").unwrap();
    mac.update(body);
    let signature = hex::encode(mac.finalize().into_bytes());
    let mut pipelined = format!("POST /github/events HTTP/1.1\r\nHost: first\r\nHost: second\r\nContent-Length:\t{} \t\r\nX-GitHub-Event: ping\r\nX-GitHub-Delivery: pipeline-first\r\nX-Hub-Signature-256: sha256={signature}\r\n\r\n", body.len()).into_bytes();
    pipelined.extend_from_slice(body);
    pipelined.extend_from_slice(b"POST /github/events HTTP/1.1\r\nContent-Length: 0\r\nX-GitHub-Delivery: pipeline-second\r\n\r\n");
    let reply = raw_http(address, &pipelined);
    assert!(reply.starts_with("HTTP/1.1 202"), "{reply}");
    assert_eq!(reply.matches("HTTP/1.1").count(), 1);
    control.stop();
    handle.join().unwrap().unwrap();
    assert_eq!(
        Store::open(settings)
            .unwrap()
            .pending_ingress()
            .unwrap()
            .len(),
        8
    );
}
struct WaitingSource(mpsc::Receiver<()>);
impl Source for WaitingSource {
    fn run(self, _sender: mpsc::Sender<Input>) -> tl3_controller::Result<()> {
        self.0.recv().unwrap();
        Ok(())
    }
}
struct FailingSource(bool);
impl Source for FailingSource {
    fn run(self, _sender: mpsc::Sender<Input>) -> tl3_controller::Result<()> {
        if self.0 {
            panic!("fixture source panic");
        }
        Err(tl3_controller::Error::Invalid("fixture source error"))
    }
}
fn source_failures(settings: &tl3_config::Store) {
    for panics in [false, true] {
        let settings = settings.clone();
        let (release, waiting) = mpsc::channel();
        let (done, result) = mpsc::channel();
        let handle = thread::spawn(move || {
            let root = tempfile::tempdir().unwrap();
            let mut controller = Controller::open(
                Store::open(&settings).unwrap(),
                adapters::simulation(root.path()).unwrap(),
                bootstrap(),
            )
            .unwrap();
            let mut sources = Sources::default();
            sources.register(WaitingSource(waiting));
            sources.register(FailingSource(panics));
            let result = sources.run(&mut controller);
            drop(controller);
            done.send(result.map_err(|error| error.to_string()))
                .unwrap();
        });
        let ending = result.recv_timeout(Duration::from_secs(10));
        release.send(()).unwrap();
        handle.join().unwrap();
        let error = ending
            .expect("one failed source must stop the loop while another remains alive")
            .unwrap_err();
        assert!(
            error.contains(if panics {
                "ingress source panicked"
            } else {
                "fixture source error"
            }),
            "{error}"
        );
    }
}
fn delivery_reconnects(settings: &tl3_config::Store, database: &mut support::Database) {
    let (address, control, handle) = start_receiver(settings);
    assert_eq!(
        post(address, "before-restart", false, fixture(false), true),
        202
    );
    database.restart();
    assert_eq!(
        post(address, "after-restart", false, fixture(false), true),
        202
    );
    database.stop();
    assert_eq!(
        post(address, "during-outage", false, fixture(false), true),
        503
    );
    database.resume();
    assert_eq!(
        post(address, "during-outage", false, fixture(false), true),
        202
    );
    assert_eq!(
        post(address, "during-outage", false, fixture(false), true),
        202
    );
    control.stop();
    handle.join().unwrap().unwrap();
    assert_eq!(
        Store::open(settings)
            .unwrap()
            .pending_ingress()
            .unwrap()
            .len(),
        3
    );
}
fn event_fixture(event: &str) -> Value {
    match event {
        "pull_request" => serde_json::from_str(include_str!(
            "../../github/tests/fixtures/pull-request-merged.json"
        ))
        .unwrap(),
        "check_run" => serde_json::from_str(include_str!(
            "../../github/tests/fixtures/check-run-completed.json"
        ))
        .unwrap(),
        "check_suite" => serde_json::from_str(include_str!(
            "../../github/tests/fixtures/check-suite-completed.json"
        ))
        .unwrap(),
        "merge_group" => {
            json!({"action": "checks_requested", "repository": fixture(false)["repository"], "installation": {"id": 3}, "merge_group": {"head_sha": "abc1234", "head_ref": "refs/heads/queue/main/test", "base_sha": "def5678", "base_ref": "refs/heads/main"}})
        }
        _ => panic!("unknown fixture"),
    }
}
struct RecordingTranslator(Arc<Mutex<Vec<GitHubDelivery>>>);
impl GitHubTranslator for RecordingTranslator {
    fn translate(
        &mut self,
        delivery: &GitHubDelivery,
        _snapshot: &tl3_store::Snapshot,
    ) -> tl3_controller::Result<Vec<Input>> {
        self.0.lock().unwrap().push(delivery.clone());
        Ok(vec![Input::Observation {
            key: "translator-comment".into(),
            observation: Observation::GitHubComment {
                repository: bootstrap().repos[0].clone(),
                issue: 12,
                comment: {
                    let mut comment = fixture(true)["comment"].clone();
                    comment["id"] = json!(903);
                    serde_json::from_value(comment).unwrap()
                },
            },
        }])
    }
}
struct FailsOnce(bool);
impl GitHubTranslator for FailsOnce {
    fn translate(
        &mut self,
        _delivery: &GitHubDelivery,
        _snapshot: &tl3_store::Snapshot,
    ) -> tl3_controller::Result<Vec<Input>> {
        if std::mem::take(&mut self.0) {
            return Err(tl3_controller::Error::Invalid("fixture translation error"));
        }
        Ok(vec![])
    }
}
fn parsed_deliveries_and_translators(settings: &tl3_config::Store) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let (address, control, handle) = start_receiver(settings);
    for event in ["pull_request", "check_run", "check_suite", "merge_group"] {
        assert_eq!(
            post_event(
                address,
                event.replace('_', "-").as_str(),
                event,
                event_fixture(event),
                true
            ),
            202
        );
    }
    let mut pr_comment = fixture(true);
    pr_comment["issue"]["number"] = json!(25);
    pr_comment["issue"]["pull_request"] =
        json!({"url": "https://api.github.com/repos/owner/sandbox/pulls/25"});
    assert_eq!(
        post(address, "pr-comment", true, pr_comment.clone(), true),
        202
    );
    let mut other = pr_comment.clone();
    other["comment"]["id"] = json!(902);
    other["comment"]["user"]["id"] = json!(7);
    assert_eq!(post(address, "other-pr-comment", true, other, true), 202);
    assert_eq!(
        post_event(
            address,
            "ping",
            "ping",
            json!({"zen": "Keep the verified payload"}),
            true
        ),
        202
    );
    control.stop();
    handle.join().unwrap().unwrap();
    let mut store = Store::open(settings).unwrap();
    let pending = store.pending_ingress().unwrap();
    assert_eq!(pending.len(), 7);
    for stored in &pending[..4] {
        let payload: Value = serde_json::from_slice(&stored.payload).unwrap();
        assert_eq!(payload, event_fixture(&stored.event_name));
        assert!(
            payload.get("Webhook").is_none(),
            "only raw evidence is stored"
        );
    }
    let mut controller = controller(settings, root.path(), &api.uri());
    let calls = Arc::new(Mutex::new(vec![]));
    controller.register_github_translator("mergify-test", RecordingTranslator(calls.clone()));
    controller.register_github_translator("retry-test", FailsOnce(true));
    assert!(ingress::drain(&mut controller).is_err());
    assert_eq!(calls.lock().unwrap().len(), 7);
    assert_eq!(
        store
            .translator_cursor("github-translator:retry-test")
            .unwrap(),
        0
    );
    assert_eq!(
        store.ingress_after(0).unwrap().len(),
        7,
        "failed consumer retains evidence"
    );
    drain(&mut controller);
    assert!(
        store.ingress_after(0).unwrap().is_empty(),
        "last cursor retires evidence"
    );
    let rows = controller.snapshot().unwrap().rows;
    let comments: Vec<_> = rows
        .iter()
        .filter(|row| row.table == "github_comments")
        .collect();
    assert!(comments.is_empty());
    for id in [9, 903] {
        assert!(
            store
                .comment_retired(&format!("owner/sandbox/3/{id}"))
                .unwrap()
        );
    }
    assert!(calls.lock().unwrap().iter().any(|delivery| matches!(delivery,
        GitHubDelivery::Webhook(delivery) if matches!(&delivery.event, Some(tl3_github::webhook::Event::CommentCreated { issue, .. }) if issue.number == 25))));
    assert!(store.pending_ingress().unwrap().is_empty());
    let later = Arc::new(Mutex::new(vec![]));
    controller.register_github_translator("later", RecordingTranslator(later.clone()));
    drain(&mut controller);
    assert!(
        later.lock().unwrap().is_empty(),
        "a new translator skips history"
    );
    assert_eq!(calls.lock().unwrap().len(), 7);
    let (address, control, handle) = start_receiver(settings);
    assert_eq!(
        post_event(
            address,
            "recent",
            "ping",
            json!({"zen": "recent state"}),
            true
        ),
        202
    );
    control.stop();
    handle.join().unwrap().unwrap();
    drain(&mut controller);
    assert_eq!(later.lock().unwrap().len(), 1);
    assert_eq!(calls.lock().unwrap().len(), 8);
    assert!(store.ingress_after(0).unwrap().is_empty());
    drop(controller);
    let mut restored = self::controller(settings, root.path(), &api.uri());
    restored.register_github_translator("mergify-test", RecordingTranslator(calls.clone()));
    drain(&mut restored);
    assert_eq!(
        calls.lock().unwrap().len(),
        8,
        "consumer cursor survives restart"
    );
    assert_eq!(
        restored
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|row| row.table == "github_comments")
            .count(),
        0
    );
}
fn catch_up_preserves_evidence(settings: &tl3_config::Store) {
    let api = Server::new();
    {
        let mut state = api.state.lock().unwrap();
        state.gap = true;
        state.catch_up_evidence = true;
    }
    let root = tempfile::tempdir().unwrap();
    let catch_up = CatchUp::new(BTreeMap::from([(3, client(&api.uri()))])).unwrap();
    let mut store = Store::open(settings).unwrap();
    let mut older = fixture(false);
    older["issue"].as_object_mut().unwrap().remove("title");
    store
        .receive_ingress(
            "github-webhook",
            "older-shape",
            "issues",
            &serde_json::to_vec(&older).unwrap(),
        )
        .unwrap();
    store
        .receive_ingress(
            "github-catch-up",
            "older-catch-up",
            "catch-up",
            br#"{"CatchUp":{"repository":{}}}"#,
        )
        .unwrap();
    catch_up.pass(&mut store, &bootstrap()).unwrap();
    let pending = store.pending_ingress().unwrap();
    assert_eq!(pending.len(), 3);
    let GitHubDelivery::CatchUp { changes, .. } =
        serde_json::from_slice(&pending[2].payload).unwrap()
    else {
        panic!("catch-up missing");
    };
    assert_eq!(changes.pull_requests.len(), 1);
    assert_eq!(changes.check_runs.len(), 1);
    assert_eq!(changes.check_suites.len(), 1);
    assert_eq!(changes.comments.len(), 2);
    assert!(store.ingress_cursor("github-catch-up").unwrap().is_some());
    let mut controller = controller(settings, root.path(), &api.uri());
    let calls = Arc::new(Mutex::new(vec![]));
    controller.register_github_translator("mergify-test", RecordingTranslator(calls.clone()));
    drain(&mut controller);
    assert!(
        matches!(&calls.lock().unwrap()[0], GitHubDelivery::CatchUp { changes, .. } if changes.check_runs.len() == 1 && changes.check_suites.len() == 1 && changes.pull_requests.len() == 1)
    );
    let rows = controller.snapshot().unwrap().rows;
    let comments: Vec<_> = rows
        .iter()
        .filter(|row| row.table == "github_comments")
        .collect();
    assert!(comments.is_empty());
    for id in [9, 901, 903] {
        assert!(
            store
                .comment_retired(&format!("owner/sandbox/3/{id}"))
                .unwrap()
        );
    }
    assert_eq!(calls.lock().unwrap().len(), 1, "older shapes were skipped");
    assert!(
        store.ingress_after(0).unwrap().is_empty(),
        "skipped rows retire too"
    );
    drain(&mut controller);
    assert_eq!(calls.lock().unwrap().len(), 1);
}

fn daemon_with_workers(settings: &tl3_config::Store) {
    let package = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../workers")
        .canonicalize()
        .unwrap();
    assert!(
        Command::new(env::var_os("TL3_TEST_UV").unwrap_or_else(|| "uv".into()))
            .args(["sync", "--locked", "--python", "3.14+gil", "--project"])
            .arg(&package)
            .status()
            .expect("daemon worker test requires uv 0.10.10 on PATH or TL3_TEST_UV")
            .success()
    );
    assert!(
        Command::new(package.join(".venv/bin/python"))
            .args(["-c", "import tiktoken; tiktoken.get_encoding('o200k_base')"])
            .env("TIKTOKEN_CACHE_DIR", env::temp_dir().join("data-gym-cache"))
            .status()
            .unwrap()
            .success()
    );
    let root = tempfile::tempdir().unwrap();
    let api = Server::new();
    let (config_path, marker, runs) = worker_daemon_config(settings, root.path(), "audit-blocked");
    let worktrees = root.path().join("worktrees");
    let checkout = worktrees
        .join(hex::encode("owner"))
        .join(hex::encode("sandbox"))
        .join("issue-12-node-0");
    fs::create_dir_all(&checkout).unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&checkout)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let base = git(&["rev-parse", "HEAD"]);
    let mut store = Store::open(settings).unwrap();
    let inputs = json!({
        "issue": {"number": 12, "title": "Add a blue button", "body": "Return hello.", "author": "owner", "comments": []},
        "repo": {"readme": "Greeting library", "file_list": ["greeting.py"], "materials": [{"path": "greeting.py", "content": "def greet(): return 'hello'\n"}]}
    });
    store.put_worker_materials("0", &json!({
        "ingest": inputs, "audit": inputs, "repo": inputs["repo"],
        "maker": {
            "brief": "Return hello.", "goals": [{"id": "greeting", "description": "Return hello.", "verification": "test"}],
            "plan": {"files": ["greeting.py"], "checks": [{"name": "hello", "kind": "demonstrating", "tests": ["tests/test_greeting.py::test_hello"], "goal_ids": ["greeting"]}]}
        }
    })).unwrap();
    struct Daemon(std::process::Child);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_tl3d"))
            .arg("--config")
            .arg(&config_path)
            .arg("--simulate")
            .arg(root.path().join("fakes"))
            .args([
                "--workers",
                "--github-listen",
                "127.0.0.1:0",
                "--github-api",
            ])
            .arg(api.uri())
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let (logs, received) = mpsc::channel();
    let stderr = daemon.0.stderr.take().unwrap();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            logs.send(line.unwrap()).unwrap();
        }
    });
    let address = loop {
        let line = received.recv_timeout(Duration::from_secs(90)).unwrap();
        if let Some(address) = line.strip_prefix("GitHub webhooks listening on ") {
            break address.parse().unwrap();
        }
    };
    assert_eq!(
        post(address, "daemon-labelled", false, fixture(false), true),
        202
    );
    let wait = |condition: &mut dyn FnMut() -> bool| {
        let started = Instant::now();
        while !condition() {
            assert!(
                started.elapsed() < Duration::from_secs(90),
                "daemon did not produce expected evidence"
            );
            thread::sleep(Duration::from_millis(10));
        }
    };
    // The source's observation key proves ingress delivered the reply, even if a look also found it.
    wait(&mut || {
        store
            .events_after(tl3_store::Revision::ZERO, 1000)
            .unwrap()
            .iter()
            .any(|event| {
                event.key.contains(":worker:") && event.payload["WorkerReply"]["role"] == "ingest"
            })
    });
    wait(&mut || {
        store.worker_document("0", "briefs").unwrap().is_some()
            && store
                .worker_watches()
                .unwrap()
                .iter()
                .any(|(_, delivery)| delivery["output"]["Effect"]["kind"]["StartRun"] == "Audit")
    });
    assert_eq!(
        store.worker_document("0", "briefs").unwrap().unwrap()["brief"],
        "Fix the greeting."
    );
    // Webhooks remain live while audit waits. Redelivery must preserve one request and one ingest.
    assert_eq!(
        post(
            address,
            "daemon-labelled-again",
            false,
            fixture(false),
            true
        ),
        202
    );
    fs::write(marker.with_extension("release"), "go").unwrap();
    wait(&mut || {
        store
            .events_after(tl3_store::Revision::ZERO, 1000)
            .unwrap()
            .iter()
            .any(|event| {
                event.key.contains(":worker:") && event.payload["WorkerReply"]["role"] == "audit"
            })
    });
    wait(&mut || api.state.lock().unwrap().closed);
    wait(&mut || {
        let snapshot = store.load().unwrap();
        snapshot.pending_effects.is_empty()
            && !snapshot.rows.iter().any(|row| row.table == "watches")
    });
    assert_eq!(
        store.worker_document("0", "audits").unwrap().unwrap()["verdict"],
        "agree"
    );
    assert_eq!(
        store.worker_document("0", "plans").unwrap().unwrap()["kind"],
        "leaf"
    );
    let replies: Vec<_> = store
        .events_after(tl3_store::Revision::ZERO, 1000)
        .unwrap()
        .into_iter()
        .filter(|event| event.payload.get("WorkerReply").is_some())
        .collect();
    for role in ["ingest", "audit", "planner", "maker"] {
        assert!(
            replies
                .iter()
                .any(|event| event.payload["WorkerReply"]["role"] == role)
        );
    }
    assert_ne!(git(&["rev-parse", "HEAD"]), base);
    assert_eq!(
        fs::read_to_string(checkout.join("greeting.py")).unwrap(),
        "def greet(): return 'hello'\n"
    );
    let snapshot = store.load().unwrap();
    assert_eq!(
        snapshot
            .rows
            .iter()
            .filter(|row| row.table == "requests")
            .count(),
        1
    );
    assert!(!snapshot.rows.iter().any(|row| row.table == "watches"));
    assert!(
        api.state
            .lock()
            .unwrap()
            .comments
            .iter()
            .any(|comment| comment["body"].as_str().unwrap().contains("Return hello."))
    );
    // Malformed durable worker notification fails the actual source while webhooks and stdin live.
    fs::write(runs.join(".ready/.ready-dead"), "{").unwrap();
    let mut ending = None;
    wait(&mut || {
        ending = daemon.0.try_wait().unwrap();
        ending.is_some()
    });
    assert_eq!(ending.unwrap().code(), Some(1));
    reader.join().unwrap();
    let logs: Vec<_> = received.try_iter().collect();
    assert!(
        logs.iter()
            .any(|line| line.contains("WorkerOutputSource failed:")),
        "{logs:?}"
    );
}

fn operations_and_delivery_log(settings: &tl3_config::Store) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let operations = tl3_config::OperationsIssue {
        owner: "owner".into(),
        name: "tl3".into(),
        installation_id: 3,
        number: 17,
    };
    let mut boot = bootstrap();
    boot.operations = Some(operations.clone());
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(Upstream::GitHub, adapter(&api.uri(), root.path()));
    let mut controller = Controller::open(Store::open(settings).unwrap(), registry, boot).unwrap();
    api.state.lock().unwrap().fail_reads = true;
    controller.look_now().unwrap();
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "delivery_looks" && r.id == "github_started")
    );
    api.state.lock().unwrap().fail_reads = false;
    assert!(
        api.state.lock().unwrap().redeliveries.is_empty(),
        "first start skips retained failures"
    );
    api.state.lock().unwrap().new_delivery = true;
    controller.look_now().unwrap();
    assert_eq!(api.state.lock().unwrap().redeliveries, vec![22]);
    controller.look_now().unwrap();
    assert_eq!(
        api.state.lock().unwrap().redeliveries,
        vec![22],
        "one recovery per failed delivery"
    );
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "delivery_looks" && r.value == 22)
    );
    let mut delivery = delivery(Notice::Incident { needs_rob: true });
    delivery.key = "incident-fixture".into();
    delivery.repository.name = "tl3".into();
    delivery.issue = 17;
    delivery.question_context = Some(
        json!({"issue_title":"Fix a greeting", "request":"Fix a greeting (https://github.com/owner/sandbox/issues/12)", "signature":"GitHub:permission"}),
    );
    delivery.note = Some(
        "For Fix a greeting, GitHub refused access. Work needs your help. Reply with \"clear incident 5.0\" here once the cause is fixed."
            .into(),
    );
    delivery.note = Some(format!(
        "{} Request: https://github.com/owner/sandbox/issues/12",
        delivery.note.unwrap()
    ));
    assert!(
        adapters::github::Adapter::new(
            BTreeMap::from([(3, client(&api.uri()))]),
            FakeWriter,
            0,
            Policy::default(),
            root.path(),
            None
        )
        .is_err()
    );
    let mut adapter = adapter(&api.uri(), root.path());
    assert_eq!(adapter.recover_deliveries(17, 0).unwrap(), Some(22));
    assert_eq!(api.state.lock().unwrap().redeliveries, vec![22, 22, 20, 18]);
    assert!(adapter.probe(&delivery).unwrap());
    adapter.execute(&delivery).unwrap();
    adapter.execute(&delivery).unwrap();
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Found(_)
    ));
    assert_eq!(api.state.lock().unwrap().posts, 1);
    let requests = api.requests();
    let post = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "POST" && r.url.path() == "/repos/owner/tl3/issues/17/comments"
        })
        .unwrap();
    let body: Value = serde_json::from_slice(&post.body).unwrap();
    let text = body["body"].as_str().unwrap();
    for expected in [
        "Fix a greeting",
        "https://github.com/owner/sandbox/issues/12",
        "GitHub",
        "refused access",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
}

fn steering_maker_worktree(root: &Path) {
    let checkout = root.join("worktrees/6f776e6572/73616e64626f78/issue-12-node-0");
    fs::create_dir_all(&checkout).unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&checkout)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "--quiet", "--template="]);
    fs::write(
        checkout.join("greeting.py"),
        "def greet(): return 'goodbye'\n",
    )
    .unwrap();
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "Base"]);
}

fn worker_daemon_config(
    settings: &tl3_config::Store,
    root: &Path,
    mode: &str,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let package = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../workers")
        .canonicalize()
        .unwrap();
    steering_maker_worktree(root);
    let credentials = root.join("app.json");
    let model_key = root.join("model-key");
    fs::write(
        &credentials,
        serde_json::to_vec(&json!({
            "id": 123, "slug": "factory", "pem": *PEM, "webhook_secret": "test-secret"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(&model_key, "fixture-only").unwrap();
    use std::os::unix::fs::PermissionsExt;
    for path in [&credentials, &model_key] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let marker = root.join("entered");
    let runs = root.join("runs");
    let config = include_str!("../../config/example.toml")
        .replace("port = 5432", &format!("port = {}", settings.port))
        .replace(
            "database = \"tl3\"",
            &format!("database = {:?}", settings.database),
        )
        .replace("user = \"tl3\"", "user = \"postgres\"")
        .replace(
            "connect_timeout_secs = 10",
            &format!(
                "password_file = {:?}\nconnect_timeout_secs = 10",
                settings.password_file.as_ref().unwrap()
            ),
        )
        .replace(
            "/home/rob/.config/tl3/github-app.json",
            credentials.to_str().unwrap(),
        )
        .replace("/run/tl3/model-key", model_key.to_str().unwrap())
        .replace("owner = \"RobTand\"", "owner = \"owner\"")
        .replace("name = \"tl3-sandbox\"", "name = \"sandbox\"")
        .replace("installation_id = 1", "installation_id = 3")
        .replace("id = 24923402", "id = 42")
        .replace(
            "/srv/tl3/workers/.venv/bin/python",
            package.join(".venv/bin/python").to_str().unwrap(),
        )
        .replace("/srv/tl3/worker-runs", runs.to_str().unwrap())
        .replace(
            "/srv/tl3/worktrees",
            root.join("worktrees").to_str().unwrap(),
        )
        .replace(
            "command = [\"-m\", \"tl3_workers\", \"run\"]",
            &format!(
                "command = [{:?}, {mode:?}, {marker:?}]",
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/worker_model.py")
            ),
        );
    let config_path = root.join("config.toml");
    let config = format!(
        "{config}\n[github.operations]\nowner = \"owner\"\nname = \"tl3\"\ninstallation_id = 3\nnumber = 17\n"
    );
    fs::write(&config_path, config).unwrap();
    (config_path, marker, runs)
}
