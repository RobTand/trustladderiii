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
    env,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, LazyLock, Mutex, mpsc},
    thread,
    time::Duration,
};
use tl3_controller::{
    Bootstrap, Boundary, Controller, LabelledIssue, Repository, adapters,
    adapters::workers::writer::{
        FakeWriter, MessageKind, WriteRequest, Writer, WriterReply, checked_draft,
    },
    executor::{Delivery, Executor, Reconciliation, Upstream},
    ingress::{self, CatchUp, Source, Webhooks},
};
use tl3_github::{AppConfig, ClientOptions, GitHub, message::Policy};
use tl3_kernel::{NodeId, Notice, Out, Outcome, Phase};
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
        actor: 42,
        label: "tl3".into(),
    }
}
fn client(uri: &str) -> GitHub {
    GitHub::new(
        AppConfig {
            id: 123,
            slug: "factory".into(),
            pem: PEM.clone(),
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
}
#[derive(Clone)]
struct Api(Arc<Mutex<ApiState>>);
impl Respond for Api {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.0.lock().unwrap();
        let path = request.url.path();
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
            ("GET", "/repos/owner/sandbox/issues/12/comments") => {
                ResponseTemplate::new(200).set_body_json(json!(&state.comments))
            }
            ("POST", "/repos/owner/sandbox/issues/12/comments") => {
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
            ("PATCH", "/repos/owner/sandbox/issues/12") => {
                state.closed = true;
                let mut issue = fixture(false)["issue"].clone();
                issue["state"] = json!("closed");
                ResponseTemplate::new(200).set_body_json(issue)
            }
            ("GET", "/repos/owner/sandbox/issues/12") => {
                let mut issue = fixture(false)["issue"].clone();
                if state.closed {
                    issue["state"] = json!("closed");
                }
                ResponseTemplate::new(200).set_body_json(issue)
            }
            ("GET", "/repos/owner/sandbox/issues") => {
                state.catch_up_passes += 1;
                ResponseTemplate::new(200).set_body_json(json!([fixture(false)["issue"]]))
            }
            ("GET", "/repos/owner/sandbox/issues/comments") => ResponseTemplate::new(200)
                .set_body_json(if state.gap {
                    {
                        let mut comment = fixture(true)["comment"].clone();
                        comment["created_at"] = json!(Utc::now());
                        comment["updated_at"] = comment["created_at"].clone();
                        json!([comment])
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
                ResponseTemplate::new(200).set_body_json(json!([]))
            }
            ("GET", path) if path.ends_with("/check-runs") => {
                ResponseTemplate::new(200).set_body_json(json!({"check_runs": []}))
            }
            ("GET", path) if path.ends_with("/check-suites") => {
                ResponseTemplate::new(200).set_body_json(json!({"check_suites": []}))
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
    adapters::github::Adapter::new(
        BTreeMap::from([(3, client(uri))]),
        FakeWriter,
        0,
        Policy {
            known_paths: ["workers/src".into()].into(),
            ..Policy::default()
        },
        root,
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
    write!(stream, "POST /webhooks/github HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nX-GitHub-Event: {}\r\nX-GitHub-Delivery: {id}\r\nX-Hub-Signature-256: sha256={signature}\r\n\r\n", body.len(), if comment { "issue_comment" } else { "issues" }).unwrap();
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
    let mut database = support::Database::start();
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
    database.stop();
    println!("5 GitHub integration scenarios passed; no skips; no live GitHub calls");
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
        1
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
    let _ = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    control.stop();
    handle.join().unwrap().unwrap();
    let mut store = Store::open(settings).unwrap();
    assert!(store.ingress_cursor("github-catch-up").unwrap().is_some());
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
        1
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
        1
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
        key: "message-key".into(),
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
        question: None,
    }
}
fn message_reconciliation() {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut adapter = adapter(&api.uri(), root.path());
    let note = delivery(Notice::MeasuresPosted);
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
    let mut old = note.clone();
    old.epoch = 1;
    assert!(adapter.execute(&old).is_err());
    assert!(adapter.reconcile(&old).is_err());
    let mut summary = delivery(Notice::Summary(Outcome::Replaced));
    summary.key = "replacement".into();
    adapter.execute(&summary).unwrap();
    assert!(!api.state.lock().unwrap().closed);
    let mut summary = delivery(Notice::Summary(Outcome::Stopped));
    summary.key = "stopped".into();
    adapter.execute(&summary).unwrap();
    assert!(api.state.lock().unwrap().closed);
    api.state.lock().unwrap().closed = false;
    assert!(matches!(
        adapter.reconcile(&summary).unwrap(),
        Reconciliation::Found(_)
    ));
    assert!(api.state.lock().unwrap().closed);
    let mut pr = note;
    pr.key = "fake-pr".into();
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
        if calls.len() <= self.failures
            && let WriterReply::Question(question) = &mut reply
        {
            question.obstacle = "The leaf needs attention.".into();
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
}
