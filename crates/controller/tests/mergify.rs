#[path = "../../store/tests/support/mod.rs"]
mod support;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};
use tl3_controller::{
    Bootstrap, Boundary, Controller, LabelledIssue, Repository, adapters,
    adapters::mergify::{Adapter, Translator},
    executor::{Delivery, Executor, Reconciliation, Upstream},
    ingress::{self, CatchUp, Source, Webhooks},
};
use tl3_github::{AppConfig, ClientOptions, GitHub};
use tl3_kernel::{Ack, EffectKind, NodeId, Out, Outcome, Phase};
use tl3_store::Store;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

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
fn fixture(name: &str, part: &str) -> Value {
    let path = format!(
        "{}/../mergify/tests/fixtures/{name}/{part}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
fn webhook_fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../github/tests/fixtures/pull-request-merged.json"
    ))
    .unwrap()
}
fn client(uri: &str) -> GitHub {
    GitHub::new(
        AppConfig {
            id: 123,
            slug: "factory".into(),
            pem: include_str!("../../mergify/tests/fixtures/throwaway-key.pem").into(),
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
struct State {
    pull: Value,
    checks: Value,
    label: bool,
    posts: usize,
    fail_reads: bool,
    lag: bool,
    fail_post: bool,
    events: Vec<Value>,
    merge_checks: Value,
}
impl State {
    fn new() -> Self {
        let mut pull = webhook_fixture()["pull_request"].clone();
        pull["number"] = json!(100);
        pull["state"] = json!("open");
        pull["merged"] = json!(false);
        pull["merged_at"] = Value::Null;
        pull["merge_commit_sha"] = json!("dddddddddddddddddddddddddddddddddddddddd");
        Self {
            pull,
            checks: fixture("waiting", "checks"),
            label: false,
            posts: 0,
            fail_reads: false,
            lag: false,
            fail_post: false,
            events: vec![],
            merge_checks: json!([{ "id": 900, "name": "CI", "head_sha": "dddddddddddddddddddddddddddddddddddddddd",
                "status": "completed", "conclusion": "success", "app": {"id": 2, "slug": "ci"} }]),
        }
    }
    fn checks(&self) -> Value {
        let mut checks = self.checks.clone();
        for check in checks.as_array_mut().unwrap() {
            check["head_sha"] = self.pull["head"]["sha"].clone();
        }
        checks
    }
    fn merged(&mut self, unapproved: bool) {
        self.pull["state"] = json!("closed");
        self.pull["merged"] = json!(true);
        self.pull["merged_at"] = json!("2026-10-30T01:00:00Z");
        if unapproved {
            self.pull["head"]["sha"] = json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        }
        self.checks = fixture("merged", "checks");
    }
}
#[derive(Clone)]
struct Api(Arc<Mutex<State>>);
impl Respond for Api {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.0.lock().unwrap();
        let path = request.url.path();
        let method = request.method.as_str();
        if path == "/app/installations/3/access_tokens" {
            return ResponseTemplate::new(201).set_body_json(
                json!({"token": "disposable", "expires_at": "2099-01-01T00:00:00Z"}),
            );
        }
        if state.fail_reads && method == "GET" {
            return ResponseTemplate::new(503);
        }
        match (method, path) {
            ("GET", "/repos/owner/sandbox/pulls/100") => ResponseTemplate::new(200).set_body_json(state.pull.clone()),
            ("GET", "/repos/owner/sandbox/issues/100") => ResponseTemplate::new(200).set_body_json(json!({
                "number":100, "title":"change", "state":state.pull["state"], "updated_at":state.pull["updated_at"],
                "labels": if state.label {json!([{"name":"tl3-queue"}])} else {json!([])},
                "pull_request":{"url":"local"}})),
            ("POST", "/repos/owner/sandbox/issues/100/labels") => {
                assert_eq!(serde_json::from_slice::<Value>(&request.body).unwrap(), json!({"labels":["tl3-queue"]}));
                state.posts += 1;
                state.label = true;
                if !state.lag { state.checks = fixture("running", "checks"); }
                if state.fail_post { return ResponseTemplate::new(503); }
                ResponseTemplate::new(200).set_body_json(json!([{"name":"tl3-queue"}]))
            }
            ("GET", "/repos/owner/sandbox/pulls") => ResponseTemplate::new(200).set_body_json(json!([state.pull])),
            ("GET", "/repos/owner/sandbox/issues/events") => ResponseTemplate::new(200).set_body_json(json!(state.events)),
            ("GET", "/repos/owner/sandbox/issues" | "/repos/owner/sandbox/issues/comments") => ResponseTemplate::new(200).set_body_json(json!([])),
            ("GET", path) if path.ends_with("/check-suites") => ResponseTemplate::new(200).set_body_json(json!({"check_suites":[]})),
            ("GET", path) if path.ends_with("/check-runs") => ResponseTemplate::new(200).set_body_json(json!({"check_runs":
                if path.contains("/commits/dddddddddddddddddddddddddddddddddddddddd/") { state.merge_checks.clone() } else {state.checks()} })),
            _ => panic!("unexpected API request {method} {path}"),
        }
    }
}
struct Server {
    _runtime: tokio::runtime::Runtime,
    server: MockServer,
    state: Arc<Mutex<State>>,
}
impl Server {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let state = Arc::new(Mutex::new(State::new()));
        let server = runtime.block_on(async {
            let server = MockServer::start().await;
            Mock::given(|_: &Request| true)
                .respond_with(Api(state.clone()))
                .mount(&server)
                .await;
            server
        });
        Self {
            _runtime: runtime,
            server,
            state,
        }
    }
    fn clients(&self) -> BTreeMap<u64, GitHub> {
        BTreeMap::from([(3, client(&self.server.uri()))])
    }
    fn adapter(&self, root: &Path, epoch: i64) -> Adapter {
        Adapter::new(self.clients(), epoch, root).unwrap()
    }
}
fn drain(controller: &mut Controller) {
    ingress::drain(controller).unwrap();
    for _ in 0..300 {
        if !controller.drive_once().unwrap() {
            return;
        }
    }
    panic!("controller did not settle");
}
fn queued(settings: &tl3_config::Store, api: &Server, root: &Path, confirmed: bool) -> Controller {
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(Upstream::Mergify, api.adapter(&root.join("mergify"), 0));
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    controller
        .intake(LabelledIssue {
            delivery: "label-1".into(),
            owner: "owner".into(),
            repo: "sandbox".into(),
            installation: 3,
            number: 12,
            actor: 42,
            label: "tl3".into(),
        })
        .unwrap();
    for _ in 0..300 {
        let progressed = controller
            .drive_once_with(|boundary, key| {
                if boundary == Boundary::BeforeDispatch {
                    let effect = Store::open(settings).unwrap().effect(key).unwrap().unwrap();
                    let delivery: Delivery = serde_json::from_value(effect.payload).unwrap();
                    if matches!(
                        delivery.output,
                        Out::Effect {
                            kind: EffectKind::Enqueue,
                            ..
                        }
                    ) {
                        api.state.lock().unwrap().pull["head"]["sha"] = json!(delivery.head_sha);
                    }
                }
            })
            .unwrap();
        if !progressed {
            break;
        }
    }
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        if confirmed {
            Phase::Queued
        } else {
            Phase::Publishing
        }
    );
    assert_eq!(api.state.lock().unwrap().posts, 1);
    controller
}
fn post(address: SocketAddr, id: &str, event: &str, body: &Value) -> u16 {
    let body = serde_json::to_vec(body).unwrap();
    let mut mac = Hmac::<Sha256>::new_from_slice(b"test-secret").unwrap();
    mac.update(&body);
    let signature = hex::encode(mac.finalize().into_bytes());
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(stream, "POST /webhooks/github HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nX-GitHub-Event: {event}\r\nX-GitHub-Delivery: {id}\r\nX-Hub-Signature-256: sha256={signature}\r\n\r\n", body.len()).unwrap();
    stream.write_all(&body).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response.split_whitespace().nth(1).unwrap().parse().unwrap()
}
fn deliver(settings: &tl3_config::Store, api: &Server, event: &str, body: &Value) {
    let source = Webhooks::bind(
        "127.0.0.1:0".parse().unwrap(),
        Store::open(settings).unwrap(),
        bootstrap(),
        b"test-secret".to_vec(),
        None,
    )
    .unwrap()
    .with_mergify(Translator::new(api.clients()).unwrap());
    let address = source.address();
    let control = source.control();
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || source.run(sender));
    assert_eq!(post(address, "recorded-1", event, body), 202);
    assert_eq!(post(address, "recorded-1", event, body), 202);
    assert_eq!(post(address, "recorded-2", event, body), 202);
    control.stop();
    handle.join().unwrap().unwrap();
    drop(receiver);
}
fn pull_event(api: &Server, action: &str) -> Value {
    let mut body = webhook_fixture();
    body["action"] = json!(action);
    body["pull_request"] = api.state.lock().unwrap().pull.clone();
    body
}
fn catch_up(settings: &tl3_config::Store, api: &Server, controller: &mut Controller) {
    let source = CatchUp::new(api.clients()).unwrap().with_mergify().unwrap();
    for _ in 0..2 {
        let inputs = source
            .pass(&mut Store::open(settings).unwrap(), &bootstrap())
            .unwrap();
        for input in inputs {
            ingress::apply(controller, input).unwrap();
        }
        drain(controller);
    }
}
fn queue_facts(controller: &mut Controller) -> Vec<Value> {
    controller
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .filter(|row| row.table == "queue_facts")
        .map(|row| row.value)
        .collect()
}
fn terminal_before_enqueue_ack(settings: &tl3_config::Store) {
    let api = Server::new();
    api.state.lock().unwrap().lag = true;
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), false);
    api.state.lock().unwrap().checks = fixture("failed", "checks");
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_run"] = api.state.lock().unwrap().checks()[0].clone();
    deliver(settings, &api, "check_run", &body);
    ingress::drain(&mut controller).unwrap();
    controller.drive_once().unwrap();
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
    controller.drive_once().unwrap();
    assert_ne!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
    controller.drive_once().unwrap();
    assert_eq!(queue_facts(&mut controller).len(), 1);
    assert_kernel_facts(settings, 1);
}
fn watched_merge(settings: &tl3_config::Store) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    api.state.lock().unwrap().merged(false);
    drain(&mut controller);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)
    );
    deliver(settings, &api, "pull_request", &pull_event(&api, "closed"));
    drain(&mut controller);
    assert_eq!(queue_facts(&mut controller).len(), 1);
    assert_kernel_facts(settings, 1);
}
fn merged(
    settings: &tl3_config::Store,
    unapproved: bool,
    webhook_first: bool,
    checks_passed: bool,
) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    api.state.lock().unwrap().merged(unapproved);
    if !checks_passed {
        api.state.lock().unwrap().merge_checks = json!([]);
    }
    if !webhook_first {
        catch_up(settings, &api, &mut controller);
    }
    deliver(settings, &api, "pull_request", &pull_event(&api, "closed"));
    drain(&mut controller);
    catch_up(settings, &api, &mut controller);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)
    );
    let facts = queue_facts(&mut controller);
    assert_eq!(facts.len(), 1);
    assert_eq!(
        facts[0]["evidence"]["Merged"]["checks_passed"],
        checks_passed
    );
    if unapproved {
        assert_eq!(
            facts[0]["evidence"]["Merged"]["head"],
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
    }
    if unapproved || !checks_passed {
        let effects = Store::open(settings).unwrap().effects().unwrap();
        assert!(effects.iter().any(
            |effect| effect.payload["output"]["Notice"]["notice"]["Note"] == "UnapprovedMerge"
        ));
    }
    assert_eq!(api.state.lock().unwrap().posts, 1);
    let applied = Store::open(settings)
        .unwrap()
        .events_after(tl3_store::Revision::ZERO, 2000)
        .unwrap()
        .into_iter()
        .filter(|event| {
            event.receipt.result["kernel_event"]["Fact"]["fact"]
                .get("PrMerged")
                .is_some()
        })
        .count();
    assert_eq!(applied, 1, "merge must step the kernel once");
    drop(controller);
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Mergify,
        api.adapter(&root.path().join("mergify"), 0),
    );
    let mut restarted =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    catch_up(settings, &api, &mut restarted);
    assert_eq!(queue_facts(&mut restarted).len(), 1);
}
fn queue_failure(settings: &tl3_config::Store, conflict: bool, infra: bool) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    let head = api.state.lock().unwrap().pull["head"]["sha"].clone();
    {
        let mut state = api.state.lock().unwrap();
        state.checks = fixture(if conflict { "conflict" } else { "failed" }, "checks");
        if infra {
            // Synthetic timeout variant of the recorded queue check.
            state.checks[0]["conclusion"] = json!("timed_out");
        }
    }
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_run"] = api.state.lock().unwrap().checks()[0].clone();
    if infra {
        body["check_suite"] =
            json!({"id":700,"head_sha":head,"status":"completed","conclusion":"timed_out"});
        deliver(settings, &api, "check_suite", &body);
    } else {
        deliver(settings, &api, "check_run", &body);
    }
    // Catch-up and the enqueue watch see the same upstream check before either is consumed.
    let inputs = CatchUp::new(api.clients())
        .unwrap()
        .with_mergify()
        .unwrap()
        .pass(&mut Store::open(settings).unwrap(), &bootstrap())
        .unwrap();
    for input in inputs {
        ingress::apply(&mut controller, input).unwrap();
    }
    ingress::drain(&mut controller).unwrap();
    // Consume just the queue fact. Subsequent maker/rebase work is outside this scenario.
    assert!(controller.drive_once().unwrap());
    let facts = queue_facts(&mut controller);
    assert_eq!(facts.len(), 1);
    assert_kernel_facts(settings, 1);
    if !conflict {
        assert_eq!(facts[0]["evidence"]["CiFailed"]["infra"], infra);
    }
    assert_ne!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
    assert_eq!(api.state.lock().unwrap().pull["head"]["sha"], head);
}
fn person_actions(settings: &tl3_config::Store, action: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    let mut body = pull_event(&api, action);
    match action {
        "unlabeled" => {
            body["label"] = json!({"name":"tl3-queue"});
            let mut state = api.state.lock().unwrap();
            state.label = false;
            state.checks = fixture("waiting", "checks");
            state.events.push(
                json!({"id":500,"event":"unlabeled", "actor":body["sender"], "label":body["label"],
                "created_at":body["pull_request"]["updated_at"],"issue":{"number":100}}),
            );
        }
        "closed" => {
            body["pull_request"]["state"] = json!("closed");
            let mut state = api.state.lock().unwrap();
            state.pull = body["pull_request"].clone();
            state
                .events
                .push(json!({"id":501,"event":"closed","actor":body["sender"],
                "created_at":body["pull_request"]["updated_at"],"issue":{"number":100}}));
        }
        "synchronize" => {
            body["before"] = body["pull_request"]["head"]["sha"].clone();
            body["after"] = json!("cccccccccccccccccccccccccccccccccccccccc");
            body["pull_request"]["head"]["sha"] = body["after"].clone();
            api.state.lock().unwrap().pull = body["pull_request"].clone();
        }
        _ => unreachable!(),
    }
    deliver(settings, &api, "pull_request", &body);
    let inputs = CatchUp::new(api.clients())
        .unwrap()
        .with_mergify()
        .unwrap()
        .pass(&mut Store::open(settings).unwrap(), &bootstrap())
        .unwrap();
    for input in inputs {
        ingress::apply(&mut controller, input).unwrap();
    }
    ingress::drain(&mut controller).unwrap();
    controller.drive_once().unwrap();
    assert_eq!(queue_facts(&mut controller).len(), 1);
    assert_kernel_facts(settings, 1);
    if action == "unlabeled" {
        assert!(controller.world().unwrap().nodes[0].paused);
    }
}
fn ignored_events(settings: &tl3_config::Store) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    let translator = Translator::new(api.clients()).unwrap();
    for action in ["closed", "synchronize", "unlabeled"] {
        let mut body = pull_event(&api, action);
        body["pull_request"]["head"]["ref"] = json!("mergify/merge-queue/main/pr-100");
        body["pull_request"]["user"]["login"] = json!("mergify[bot]");
        body["before"] = json!("before");
        body["after"] = body["pull_request"]["head"]["sha"].clone();
        body["label"] = json!({"name":"tl3-queue"});
        let event = parsed("pull_request", body);
        assert!(
            translator
                .webhook(&event, &controller.snapshot().unwrap(), &bootstrap())
                .unwrap()
                .is_empty()
        );
    }
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_suite"] =
        json!({"id":77,"head_sha":"draft-head","status":"completed","conclusion":"failure"});
    assert!(
        translator
            .webhook(
                &parsed("check_suite", body.clone()),
                &controller.snapshot().unwrap(),
                &bootstrap()
            )
            .unwrap()
            .is_empty()
    );
    body["check_suite"]["head_sha"] = api.state.lock().unwrap().pull["head"]["sha"].clone();
    body["check_suite"]["pull_requests"] = json!([{"number":999}]);
    assert!(
        translator
            .webhook(
                &parsed("check_suite", body.clone()),
                &controller.snapshot().unwrap(),
                &bootstrap()
            )
            .unwrap()
            .is_empty()
    );
    body["check_suite"]["pull_requests"] = json!([]);
    body["installation"]["id"] = json!(4);
    assert!(
        translator
            .webhook(
                &parsed("check_suite", body),
                &controller.snapshot().unwrap(),
                &bootstrap()
            )
            .unwrap()
            .is_empty()
    );
    let mut body = webhook_fixture();
    body["action"] = json!("checks_requested");
    body["merge_group"] = json!({"head_sha":"draft-head","head_ref":"gh-readonly-queue/main/pr-100","base_ref":"refs/heads/main"});
    assert!(
        translator
            .webhook(
                &parsed("merge_group", body),
                &controller.snapshot().unwrap(),
                &bootstrap()
            )
            .unwrap()
            .is_empty()
    );
    assert!(queue_facts(&mut controller).is_empty());
}
fn assert_kernel_facts(settings: &tl3_config::Store, expected: usize) {
    let count = Store::open(settings)
        .unwrap()
        .events_after(tl3_store::Revision::ZERO, 2000)
        .unwrap()
        .into_iter()
        .filter(|event| event.receipt.result["kernel_event"].get("Fact").is_some())
        .count();
    assert_eq!(count, expected, "each queue fact must step the kernel once");
}
fn parsed(name: &str, value: Value) -> tl3_github::webhook::Event {
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut mac = Hmac::<Sha256>::new_from_slice(b"test-secret").unwrap();
    mac.update(&bytes);
    tl3_github::webhook::verify_and_parse(
        b"test-secret",
        &format!("sha256={}", hex::encode(mac.finalize().into_bytes())),
        "parse",
        name,
        &bytes,
    )
    .unwrap()
    .event
    .unwrap()
}
fn delivery() -> Delivery {
    Delivery {
        key: "enqueue-1".into(),
        epoch: 1,
        output: Out::Effect {
            node: NodeId(0),
            seq: 1,
            kind: EffectKind::Enqueue,
            head: None,
        },
        generation: 1,
        head_sha: Some("abc1234".into()),
        repository: bootstrap().repos.remove(0),
        issue: 12,
        pull_request: Some(100),
        paths: vec![],
        attempt: 1,
        round: 0,
        question: None,
    }
}
fn executor_recovery() {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut adapter = api.adapter(root.path(), 1);
    let mut delivery = delivery();
    api.state.lock().unwrap().lag = true;
    assert!(adapter.execute(&delivery).is_err());
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Pending
    ));
    assert_eq!(api.state.lock().unwrap().posts, 1);
    api.state.lock().unwrap().checks = fixture("running", "checks");
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Found(_)
    ));
    assert_eq!(adapter.execute(&delivery).unwrap().ack, Ack::Confirmed);
    assert_eq!(api.state.lock().unwrap().posts, 1);
    api.state.lock().unwrap().label = false;
    api.state.lock().unwrap().checks = fixture("waiting", "checks");
    drop(adapter);
    let mut adapter = api.adapter(root.path(), 1);
    assert_eq!(adapter.execute(&delivery).unwrap().ack, Ack::Confirmed);
    assert_eq!(api.state.lock().unwrap().posts, 1);
    api.state.lock().unwrap().fail_reads = true;
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Pending
    ));
    api.state.lock().unwrap().fail_reads = false;
    delivery.epoch = 2;
    adapter.execute(&delivery).unwrap();
    delivery.epoch = 1;
    assert!(adapter.execute(&delivery).is_err());
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Found(_)
    ));
    delivery.epoch = 2;
    delivery.key = "retry-enqueue-2".into();
    api.state.lock().unwrap().checks = fixture("failed", "checks");
    api.state.lock().unwrap().label = true;
    let posts = api.state.lock().unwrap().posts;
    api.state.lock().unwrap().lag = true;
    assert!(adapter.execute(&delivery).is_err());
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Pending
    ));
    api.state.lock().unwrap().checks = fixture("running", "checks");
    assert_eq!(adapter.execute(&delivery).unwrap().ack, Ack::Confirmed);
    assert_eq!(
        api.state.lock().unwrap().posts,
        posts + 1,
        "new key must enqueue after the old failure"
    );
    delivery.key = "lost-label-response".into();
    api.state.lock().unwrap().checks = fixture("waiting", "checks");
    api.state.lock().unwrap().label = false;
    api.state.lock().unwrap().lag = false;
    api.state.lock().unwrap().fail_post = true;
    let posts = api.state.lock().unwrap().posts;
    assert!(adapter.execute(&delivery).is_err());
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Found(_)
    ));
    assert_eq!(api.state.lock().unwrap().posts, posts + 1);
    delivery.epoch = 1;
    drop(adapter);
    let mut old = api.adapter(root.path(), 1);
    assert!(old.execute(&delivery).is_err());
}
fn main() {
    executor_recovery();
    println!("test executor_recovery ... ok");
    let mut database = support::Database::start();
    merged(&database.create("merged_webhook"), false, true, true);
    merged(&database.create("merged_catchup"), false, false, true);
    merged(&database.create("unapproved"), true, true, true);
    merged(&database.create("missing_merge_ci"), false, true, false);
    watched_merge(&database.create("watched"));
    terminal_before_enqueue_ack(&database.create("late_enqueue_ack"));
    println!("test merged, catch-up, restart and unapproved head ... ok");
    queue_failure(&database.create("failed"), false, false);
    queue_failure(&database.create("infra"), false, true);
    queue_failure(&database.create("conflict"), true, false);
    println!("test queue failures and conflict ... ok");
    for action in ["unlabeled", "closed", "synchronize"] {
        person_actions(&database.create(action), action);
    }
    println!("test person actions ... ok");
    ignored_events(&database.create("ignored"));
    println!("test ignored events ... ok");
    database.stop();
    println!("14 Mergify scenarios passed; no skips; no live upstream calls");
}
