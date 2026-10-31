mod support;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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
    executor::{Delivery, Executor, Output as Out, Reconciliation, Upstream},
    ingress::{self, CatchUp, Source, Webhooks},
};
use tl3_github::{AppConfig, ClientOptions, GitHub};
use tl3_kernel::{Ack, EffectKind, NodeId, Outcome, Phase};
use tl3_store::Store;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

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
    rate_limit: Option<u16>,
    http_failure: Option<u16>,
    lag: bool,
    fail_post: bool,
    events: Vec<Value>,
    merge_checks: Value,
    deletes: usize,
    remove_after_post: bool,
    pull_reads: usize,
    reads: usize,
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
            rate_limit: None,
            http_failure: None,
            lag: false,
            fail_post: false,
            events: vec![],
            merge_checks: json!([]),
            deletes: 0,
            remove_after_post: false,
            pull_reads: 0,
            reads: 0,
        }
    }
    fn checks(&self) -> Value {
        let mut checks = self.checks.clone();
        for check in checks.as_array_mut().unwrap() {
            check["head_sha"] = self.pull["head"]["sha"].clone();
        }
        checks
    }
    fn queue_check(&self) -> Value {
        self.checks()
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["name"] == tl3_mergify::QUEUE_CHECK)
            .unwrap()
            .clone()
    }
    fn queue_check_mut(&mut self) -> &mut Value {
        self.checks
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|check| check["name"] == tl3_mergify::QUEUE_CHECK)
            .unwrap()
    }
    fn merged(&mut self, unapproved: bool) {
        self.pull["state"] = json!("closed");
        self.pull["merged"] = json!(true);
        self.pull["merged_at"] = json!("2026-10-30T01:00:00Z");
        if unapproved {
            self.pull["head"]["sha"] = json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        }
    }
}
#[derive(Clone)]
struct Api(Arc<Mutex<State>>);
impl support::github::Routes for State {
    fn pulls(&self) -> Value {
        json!([self.pull])
    }
    fn checks(&self, sha: &str) -> Value {
        if sha == "dddddddddddddddddddddddddddddddddddddddd" {
            self.merge_checks.clone()
        } else {
            self.checks()
        }
    }
}
impl Respond for Api {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.0.lock().unwrap();
        let path = request.url.path();
        let method = request.method.as_str();
        if let Some(response) = support::github::access_token(path, 3, "disposable") {
            return response;
        }
        if method == "GET" {
            state.reads += 1;
        }
        if let Some(status) = state.http_failure.filter(|_| method == "GET") {
            return ResponseTemplate::new(status)
                .set_body_json(json!({"message": "fixture failure"}));
        }
        if let Some(status) = state.rate_limit.filter(|_| method == "GET") {
            return ResponseTemplate::new(status)
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("retry-after", "600")
                .set_body_json(json!({"message": "API rate limit exceeded"}));
        }
        if state.fail_reads && method == "GET" {
            return ResponseTemplate::new(503);
        }
        if let Some(response) =
            support::github::respond(request, path, "/repos/owner/sandbox", &mut *state)
        {
            return response;
        }
        match (method, path) {
            ("GET", "/repos/owner/sandbox/pulls/100") => {
                state.pull_reads += 1;
                ResponseTemplate::new(200).set_body_json(state.pull.clone())
            },
            ("GET", "/repos/owner/sandbox/issues/100") => ResponseTemplate::new(200).set_body_json(json!({
                "number":100, "title":"change", "state":state.pull["state"], "updated_at":state.pull["updated_at"],
                "labels": if state.label {json!([{"name":"tl3-queue"}])} else {json!([])},
                "pull_request":{"url":"local"}})),
            ("POST", "/repos/owner/sandbox/issues/100/labels") => {
                assert_eq!(serde_json::from_slice::<Value>(&request.body).unwrap(), json!({"labels":["tl3-queue"]}));
                state.posts += 1;
                let changed = !state.label;
                state.label = true;
                if changed && !state.lag { state.checks = fixture("running", "checks"); }
                if state.remove_after_post {
                    state.label = false;
                    state.events.push(json!({"id":600,"event":"unlabeled","actor":{"id":42,"login":"rob"},
                        "label":{"name":"tl3-queue"},"created_at":"2026-10-30T02:00:00Z","issue":{"number":100}}));
                }
                if state.fail_post { return ResponseTemplate::new(503); }
                ResponseTemplate::new(200).set_body_json(json!([{"name":"tl3-queue"}]))
            }
            ("DELETE", "/repos/owner/sandbox/issues/100/labels/tl3-queue") => {
                state.deletes += 1;
                state.label = false;
                ResponseTemplate::new(200).set_body_json(json!([]))
            }
            ("GET", "/repos/owner/sandbox/issues/100/events") => ResponseTemplate::new(200).set_body_json(json!(state.events)),
            ("GET", "/repos/owner/sandbox/issues/events") => ResponseTemplate::new(200).set_body_json(json!(state.events)),
            ("GET", "/repos/owner/sandbox/issues" | "/repos/owner/sandbox/issues/comments") => ResponseTemplate::new(200).set_body_json(json!([])),
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
    controller.register_github_translator("mergify", Translator::new(api.clients()).unwrap());
    controller
        .intake(LabelledIssue {
            delivery: "label-1".into(),
            owner: "owner".into(),
            repo: "sandbox".into(),
            installation: 3,
            number: 12,
            title: "change".into(),
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
    write!(stream, "POST /github/events HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nX-GitHub-Event: {event}\r\nX-GitHub-Delivery: {id}\r\nX-Hub-Signature-256: sha256={signature}\r\n\r\n", body.len()).unwrap();
    stream.write_all(&body).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response.split_whitespace().nth(1).unwrap().parse().unwrap()
}
fn deliver(settings: &tl3_config::Store, _api: &Server, event: &str, body: &Value) {
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
    let handle = thread::spawn(move || source.run(sender));
    let identity = format!(
        "{}-{}",
        event.replace('_', "-"),
        hex::encode(Sha256::digest(serde_json::to_vec(body).unwrap()))
    );
    let first = format!("{identity}-1");
    let second = format!("{identity}-2");
    assert_eq!(post(address, &first, event, body), 202);
    assert_eq!(post(address, &first, event, body), 202);
    assert_eq!(post(address, &second, event, body), 202);
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
    let source = CatchUp::new(api.clients()).unwrap();
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
    body["check_run"] = api.state.lock().unwrap().queue_check();
    deliver(settings, &api, "check_run", &body);
    ingress::drain(&mut controller).unwrap();
    let mut boundaries = Vec::new();
    controller
        .drive_once_with(|boundary, key| boundaries.push((boundary, key.to_owned())))
        .unwrap();
    assert_eq!(boundaries.len(), 2);
    assert_eq!(boundaries[0].0, Boundary::BeforeReconcile);
    assert_eq!(boundaries[1].0, Boundary::AfterReconcile);
    assert_eq!(boundaries[0].1, boundaries[1].1);
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
    api.state.lock().unwrap().checks = fixture("merged", "checks");
    drain(&mut controller);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)
    );
    deliver(settings, &api, "pull_request", &pull_event(&api, "closed"));
    drain(&mut controller);
    assert!(queue_facts(&mut controller).is_empty());
    let reads = api.state.lock().unwrap().pull_reads;
    catch_up(settings, &api, &mut controller);
    let mut body = webhook_fixture();
    body["action"] = json!("checks_requested");
    body["merge_group"] = json!({"head_sha":"batch","head_ref":"gh-readonly-queue/main/pr-100","base_ref":"main","base_sha":"base"});
    let inputs = Translator::new(api.clients())
        .unwrap()
        .webhook(
            &parsed("merge_group", body),
            &controller.snapshot().unwrap(),
            &bootstrap().repos,
        )
        .unwrap();
    assert!(inputs.is_empty());
    assert_eq!(
        api.state.lock().unwrap().pull_reads,
        reads,
        "closed nodes leave the watched set"
    );
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
    api.state.lock().unwrap().checks =
        fixture(if checks_passed { "merged" } else { "failed" }, "checks");
    // Merge-method merge creates a new commit with no completed push CI.
    assert!(
        api.state
            .lock()
            .unwrap()
            .merge_checks
            .as_array()
            .unwrap()
            .is_empty()
    );
    api.state.lock().unwrap().events.push(
        json!({"id":501,"event":"closed","actor":{"id":42,"login":"rob"},
        "created_at":"2026-10-30T02:00:00Z","issue":{"number":100}}),
    );
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
    assert!(queue_facts(&mut controller).is_empty());
    let facts: Vec<_> = Store::open(settings)
        .unwrap()
        .events_after(tl3_store::Revision::ZERO, 2000)
        .unwrap()
        .into_iter()
        .filter_map(|event| {
            event.receipt.result["kernel_event"]["Fact"]["fact"]
                .get("PrMerged")
                .cloned()
        })
        .collect();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0]["checks_passed"], checks_passed);
    if unapproved {
        assert!(
            controller
                .snapshot()
                .unwrap()
                .interned
                .iter()
                .any(|entry| entry.value == "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
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
    restarted.register_github_translator("mergify", Translator::new(api.clients()).unwrap());
    catch_up(settings, &api, &mut restarted);
    assert!(queue_facts(&mut restarted).is_empty());
    assert_kernel_facts(settings, 1);
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
            state.queue_check_mut()["conclusion"] = json!("timed_out");
        }
    }
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_run"] = api.state.lock().unwrap().queue_check();
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
                "created_at":"2026-10-30T02:00:00Z","issue":{"number":100}}),
            );
        }
        "closed" => {
            body["pull_request"]["state"] = json!("closed");
            let mut state = api.state.lock().unwrap();
            state.pull = body["pull_request"].clone();
            state
                .events
                .push(json!({"id":501,"event":"closed","actor":body["sender"],
                "created_at":"2026-10-30T02:00:00Z","issue":{"number":100}}));
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
        controller
            .observe(
                "resume",
                tl3_controller::executor::Observation::Kernel(tl3_kernel::Event::Command {
                    node: NodeId(0),
                    id: 1,
                    command: tl3_kernel::Command::Resume,
                }),
            )
            .unwrap();
        drain(&mut controller);
        assert!(!controller.world().unwrap().nodes[0].paused);
        let inputs = Translator::new(api.clients())
            .unwrap()
            .webhook(
                &parsed("pull_request", body),
                &controller.snapshot().unwrap(),
                &bootstrap().repos,
            )
            .unwrap();
        for input in inputs {
            ingress::apply(&mut controller, input).unwrap();
        }
        drain(&mut controller);
        assert!(!controller.world().unwrap().nodes[0].paused);
        assert_kernel_facts(settings, 1);
    }
}
fn ignored_events(settings: &tl3_config::Store) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    let translator = Translator::new(api.clients()).unwrap();
    for action in ["completed", "created"] {
        for (name, app) in [
            ("Mergify Merge Queue", json!({"id": 99, "slug": "other"})),
            ("Summary", json!({"id": 10562, "slug": "mergify"})),
            ("Mergify Merge Queue", Value::Null),
        ] {
            let mut body = webhook_fixture();
            body["action"] = json!(action);
            body["check_run"] = api.state.lock().unwrap().queue_check();
            body["check_run"]["name"] = json!(name);
            body["check_run"]["app"] = app;
            let reads = api.state.lock().unwrap().reads;
            assert!(
                translator
                    .webhook(
                        &parsed("check_run", body),
                        &controller.snapshot().unwrap(),
                        &bootstrap().repos,
                    )
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(api.state.lock().unwrap().reads, reads);
        }
    }
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_suite"] = json!({"id": 77, "head_sha": api.state.lock().unwrap().pull["head"]["sha"],
        "status": "completed", "conclusion": "success", "app": {"id": 10562, "slug": "mergify"}});
    let reads = api.state.lock().unwrap().reads;
    assert!(
        translator
            .webhook(
                &parsed("check_suite", body),
                &controller.snapshot().unwrap(),
                &bootstrap().repos,
            )
            .unwrap()
            .is_empty()
    );
    assert_eq!(api.state.lock().unwrap().reads, reads);
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
                .webhook(&event, &controller.snapshot().unwrap(), &bootstrap().repos)
                .unwrap()
                .is_empty()
        );
    }
    for (branch, author) in [
        ("mergify/merge-queue/main/pr-100", "human"),
        ("feature", "mergify[bot]"),
    ] {
        let mut body = pull_event(&api, "synchronize");
        body["pull_request"]["head"]["ref"] = json!(branch);
        body["pull_request"]["user"]["login"] = json!(author);
        body["before"] = json!("old");
        body["after"] = body["pull_request"]["head"]["sha"].clone();
        assert_eq!(
            translator
                .webhook(
                    &parsed("pull_request", body),
                    &controller.snapshot().unwrap(),
                    &bootstrap().repos
                )
                .unwrap()
                .len(),
            1
        );
    }
    api.state.lock().unwrap().checks = fixture("failed", "checks");
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_suite"] =
        json!({"id":77,"head_sha":"draft-head","status":"completed","conclusion":"failure"});
    assert!(
        translator
            .webhook(
                &parsed("check_suite", body.clone()),
                &controller.snapshot().unwrap(),
                &bootstrap().repos
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
                &bootstrap().repos
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
                &bootstrap().repos
            )
            .unwrap()
            .is_empty()
    );
    api.state.lock().unwrap().checks = fixture("running", "checks");
    let mut body = webhook_fixture();
    body["action"] = json!("checks_requested");
    body["merge_group"] = json!({"head_sha":"draft-head","head_ref":"gh-readonly-queue/main/pr-100","base_ref":"refs/heads/main","base_sha":"base"});
    assert!(
        translator
            .webhook(
                &parsed("merge_group", body),
                &controller.snapshot().unwrap(),
                &bootstrap().repos
            )
            .unwrap()
            .is_empty()
    );
    assert!(queue_facts(&mut controller).is_empty());
}
fn late_merge_after_close(settings: &tl3_config::Store, webhook: bool) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    controller
        .observe(
            "stop",
            tl3_controller::executor::Observation::Kernel(tl3_kernel::Event::Command {
                node: NodeId(0),
                id: 1,
                command: tl3_kernel::Command::Stop,
            }),
        )
        .unwrap();
    drain(&mut controller);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Stopped)
    );
    api.state.lock().unwrap().merged(false);
    // GitHub merges first, catch-up observes it while the queue check still runs.
    deliver(settings, &api, "pull_request", &pull_event(&api, "closed"));
    drain(&mut controller);
    catch_up(settings, &api, &mut controller);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Stopped)
    );
    api.state.lock().unwrap().checks = fixture("merged", "checks");
    if webhook {
        let mut body = webhook_fixture();
        body["action"] = json!("completed");
        body["check_run"] = api.state.lock().unwrap().queue_check();
        let inputs = Translator::new(api.clients())
            .unwrap()
            .webhook(
                &parsed("check_run", body.clone()),
                &controller.snapshot().unwrap(),
                &bootstrap().repos,
            )
            .unwrap();
        assert_eq!(
            inputs.len(),
            1,
            "closed unmerged node must translate the final check"
        );
        deliver(settings, &api, "check_run", &body);
        drain(&mut controller);
    } else {
        // This branch supplies a PR in the next catch-up window. The webhook
        // branch above covers a final check after the PR left that window.
        let cursor = Store::open(settings)
            .unwrap()
            .ingress_cursor("github-catch-up")
            .unwrap()
            .unwrap();
        api.state.lock().unwrap().pull["updated_at"] = cursor;
        catch_up(settings, &api, &mut controller);
    }
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged),
        "late merge path webhook={webhook}"
    );
    assert!(queue_facts(&mut controller).is_empty());
    assert_kernel_facts(settings, 1);
    let credit = controller.world().unwrap().nodes[0].credit;
    assert!(credit.merge_credited);
    let effects = Store::open(settings).unwrap().effects().unwrap();
    assert_eq!(
        effects
            .iter()
            .filter(|effect| effect.payload["output"]["Notice"]["notice"]["Note"] == "LateMerge")
            .count(),
        1
    );
    assert!(effects.iter().any(|effect| {
        effect.payload["output"]["Notice"]["notice"]
            .get("Incident")
            .is_some()
    }));
    catch_up(settings, &api, &mut controller);
    assert_eq!(controller.world().unwrap().nodes[0].credit, credit);
    assert_kernel_facts(settings, 1);
}

fn translator_retries_after_api_failure(settings: &tl3_config::Store) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    api.state.lock().unwrap().checks = fixture("failed", "checks");
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_run"] = api.state.lock().unwrap().queue_check();
    api.state.lock().unwrap().fail_reads = true;
    deliver(settings, &api, "check_run", &body);
    assert!(ingress::drain(&mut controller).is_err());
    assert_eq!(
        Store::open(settings)
            .unwrap()
            .translator_cursor("github-translator:mergify")
            .unwrap(),
        0
    );
    api.state.lock().unwrap().fail_reads = false;
    ingress::drain(&mut controller).unwrap();
    assert!(controller.drive_once().unwrap());
    assert_kernel_facts(settings, 1);
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
        replanned: false,
        question: None,
        marker_key: None,
        note: None,
        question_context: None,
        checks: None,
        job_generation: None,
        steering: None,
        request_text: None,
    }
}
fn label_removed_before_confirmation() {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    {
        let mut state = api.state.lock().unwrap();
        state.lag = true;
        state.remove_after_post = true;
    }
    let delivery = delivery();
    let mut adapter = api.adapter(root.path(), 1);
    let completion = adapter.execute(&delivery).unwrap();
    assert_eq!(completion.ack, Ack::Confirmed);
    assert!(
        matches!(&completion.observations[0], tl3_controller::executor::Observation::Queue(fact)
        if matches!(fact.evidence, adapters::mergify::Evidence::DequeuedByPerson { event: 600 }))
    );
    assert!(root.path().read_dir().unwrap().any(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_some_and(|extension| extension == "applied")
    }));
    drop(adapter);
    let mut adapter = api.adapter(root.path(), 1);
    assert_eq!(adapter.execute(&delivery).unwrap().ack, Ack::Confirmed);
    assert_eq!(api.state.lock().unwrap().posts, 1);
    assert!(!api.state.lock().unwrap().label);
}

fn applied_marker_waits_for_person_evidence() {
    let api = Server::new();
    api.state.lock().unwrap().lag = true;
    let root = tempfile::tempdir().unwrap();
    let delivery = delivery();
    let mut adapter = api.adapter(root.path(), 1);
    assert!(adapter.execute(&delivery).is_err());
    api.state.lock().unwrap().label = false;
    drop(adapter);
    let mut adapter = api.adapter(root.path(), 1);
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Pending
    ));
    assert!(adapter.execute(&delivery).is_err());
    assert_eq!(api.state.lock().unwrap().posts, 1);
    api.state.lock().unwrap().events.push(
        json!({"id":601,"event":"unlabeled","actor":{"id":42,"login":"rob"},
        "label":{"name":"tl3-queue"},"created_at":"2026-10-30T02:00:00Z"}),
    );
    let Reconciliation::Found(completion) = adapter.reconcile(&delivery).unwrap() else {
        panic!("person removal must confirm");
    };
    assert!(
        matches!(&completion.observations[0], tl3_controller::executor::Observation::Queue(fact)
        if matches!(fact.evidence, adapters::mergify::Evidence::DequeuedByPerson { event: 601 }))
    );
    assert_eq!(api.state.lock().unwrap().posts, 1);
}

fn in_progress_confirms_enqueue(settings: &tl3_config::Store) {
    let api = Server::new();
    api.state.lock().unwrap().lag = true;
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), false);
    api.state.lock().unwrap().checks = fixture("running", "checks");
    let mut body = webhook_fixture();
    body["action"] = json!("in_progress");
    body["check_run"] = api
        .state
        .lock()
        .unwrap()
        .checks()
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == tl3_mergify::QUEUE_CHECK)
        .unwrap()
        .clone();
    assert_eq!(body["check_run"]["status"], "in_progress");
    assert!(body["check_run"]["conclusion"].is_null());
    deliver(settings, &api, "check_run", &body);
    drain(&mut controller);
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
    assert_eq!(api.state.lock().unwrap().posts, 1);
    assert_kernel_facts(settings, 0);
}

fn outside_merge_checks(settings: &tl3_config::Store, passed: bool, has_checks: bool, stale: bool) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    {
        let mut state = api.state.lock().unwrap();
        state.merged(false);
        state.checks = if stale {
            fixture("waiting", "checks")
        } else {
            json!([])
        };
        let sha = state.pull["merge_commit_sha"].clone();
        state.merge_checks = json!([
            {"id":900,"name":"CI","head_sha":sha,"status":"completed","conclusion":if passed { "success" } else { "failure" }},
            {"id":901,"name":"optional","head_sha":sha,"status":"completed","conclusion":"skipped"},
            {"id":902,"name":"lint","head_sha":sha,"status":"completed","conclusion":"neutral"}
        ]);
        if !has_checks {
            state.merge_checks = json!([]);
        }
    }
    deliver(settings, &api, "pull_request", &pull_event(&api, "closed"));
    drain(&mut controller);
    let events = Store::open(settings)
        .unwrap()
        .events_after(tl3_store::Revision::ZERO, 2000)
        .unwrap();
    let merges: Vec<_> = events
        .iter()
        .filter_map(|event| event.receipt.result["kernel_event"]["Fact"]["fact"].get("PrMerged"))
        .collect();
    assert_eq!(merges.len(), 1);
    assert_eq!(merges[0]["checks_passed"], passed);
}

fn delayed_merge(settings: &tl3_config::Store, passed: bool, webhook: bool) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    api.state.lock().unwrap().merged(false);
    assert_eq!(
        api.state.lock().unwrap().queue_check()["status"],
        "in_progress"
    );
    deliver(settings, &api, "pull_request", &pull_event(&api, "closed"));
    drain(&mut controller);
    catch_up(settings, &api, &mut controller);
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
    assert!(controller.world().unwrap().nodes[0].merged.is_none());
    assert!(queue_facts(&mut controller).is_empty());
    assert_kernel_facts(settings, 0);

    // The merge is already recorded. Only now does Mergify finish its queue check.
    api.state.lock().unwrap().checks = fixture(if passed { "merged" } else { "failed" }, "checks");
    if webhook {
        let mut body = webhook_fixture();
        body["action"] = json!("completed");
        body["check_run"] = api.state.lock().unwrap().queue_check();
        deliver(settings, &api, "check_run", &body);
        drain(&mut controller);
    } else {
        catch_up(settings, &api, &mut controller);
    }
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)
    );
    let events = Store::open(settings)
        .unwrap()
        .events_after(tl3_store::Revision::ZERO, 2000)
        .unwrap();
    let merges: Vec<_> = events
        .iter()
        .filter_map(|event| event.receipt.result["kernel_event"]["Fact"]["fact"].get("PrMerged"))
        .collect();
    assert_eq!(merges.len(), 1);
    assert_eq!(merges[0]["checks_passed"], passed);
    let incident = Store::open(settings)
        .unwrap()
        .effects()
        .unwrap()
        .iter()
        .any(|effect| effect.payload["output"]["Notice"]["notice"]["Note"] == "UnapprovedMerge");
    assert_eq!(incident, !passed);
    catch_up(settings, &api, &mut controller);
    assert_kernel_facts(settings, 1);
}

fn progress(controller: &mut Controller) -> tl3_controller::executor::WatchProgress {
    let snapshot = controller.snapshot().unwrap();
    let watch = snapshot
        .rows
        .iter()
        .find(|row| row.table == "watches" && row.value["output"]["Effect"]["kind"] == "Enqueue")
        .unwrap();
    serde_json::from_value(watch.value["progress"].clone()).unwrap()
}

fn stalled_merge_progress(settings: &tl3_config::Store) {
    use tl3_controller::executor::{ProgressToken, WatchProgress};
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    drain(&mut controller);
    let before = progress(&mut controller).current;
    api.state.lock().unwrap().merged(false);
    assert!(!controller.drive_once().unwrap());
    let changed = progress(&mut controller);
    assert!(changed.changed());
    assert_eq!(changed.previous, Some(before));
    let ProgressToken::Mergify(token) = &changed.current;
    assert_eq!(
        token.merged_at.as_deref(),
        Some("2026-10-30T01:00:00+00:00")
    );
    assert_eq!(
        token.merge_commit_sha,
        api.state.lock().unwrap().pull["merge_commit_sha"]
            .as_str()
            .map(String::from)
    );
    let check = token.queue_check.as_ref().unwrap();
    assert_eq!(check.status, "in_progress");
    assert!(check.conclusion.is_none());
    assert_eq!(check.title.as_deref(), Some("Running merge queue checks"));
    assert_eq!(
        check.id,
        api.state.lock().unwrap().queue_check()["id"]
            .as_u64()
            .unwrap()
    );
    drop(controller);
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Mergify,
        api.adapter(&root.path().join("mergify"), 0),
    );
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    assert_eq!(progress(&mut controller), changed);
    assert!(!controller.drive_once().unwrap());
    let unchanged = progress(&mut controller);
    assert!(!unchanged.changed());
    assert_eq!(unchanged.previous.as_ref(), Some(&unchanged.current));
    assert!(
        !controller.drive_once().unwrap(),
        "unchanged tokens must not spin the driver"
    );
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
    assert_kernel_facts(settings, 0);
    // Each selected check field participates in the token comparison.
    for field in ["id", "status", "conclusion", "title"] {
        let previous = progress(&mut controller).current;
        {
            let mut state = api.state.lock().unwrap();
            match field {
                "id" => state.queue_check_mut()["id"] = json!(9999),
                "status" => state.queue_check_mut()["status"] = json!("queued"),
                "conclusion" => state.queue_check_mut()["conclusion"] = json!("neutral"),
                "title" => state.queue_check_mut()["output"]["title"] = json!("In merge queue"),
                _ => unreachable!(),
            }
        }
        assert!(!controller.drive_once().unwrap());
        let read: WatchProgress = progress(&mut controller);
        assert!(read.changed());
        assert_eq!(read.previous, Some(previous));
        drain(&mut controller);
    }
}

fn unapproved_merge_does_not_wait(settings: &tl3_config::Store, path: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    api.state.lock().unwrap().merged(true);
    if path == "current_unapproved" {
        controller
            .observe(
                "outside-push",
                tl3_controller::executor::Observation::Queue(adapters::mergify::QueueFact {
                    repository: bootstrap().repos[0].clone(),
                    number: 100,
                    evidence: adapters::mergify::Evidence::PushByOther {
                        head: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                    },
                }),
            )
            .unwrap();
        assert!(controller.drive_once().unwrap());
        let node = controller.world().unwrap().nodes[0];
        assert_ne!(
            node.head, node.approved,
            "the current head is not the approved head"
        );
    }
    match path {
        "webhook" | "current_unapproved" => {
            deliver(settings, &api, "pull_request", &pull_event(&api, "closed"))
        }
        "catchup" => catch_up(settings, &api, &mut controller),
        "watch" => (),
        _ => unreachable!(),
    }
    drain(&mut controller);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)
    );
    assert_kernel_facts(settings, if path == "current_unapproved" { 2 } else { 1 });
    let merges = Store::open(settings)
        .unwrap()
        .events_after(tl3_store::Revision::ZERO, 2000)
        .unwrap()
        .iter()
        .filter(|event| {
            event.receipt.result["kernel_event"]["Fact"]["fact"]
                .get("PrMerged")
                .is_some()
        })
        .count();
    assert_eq!(merges, 1);
    let effects = Store::open(settings).unwrap().effects().unwrap();
    assert!(
        effects.iter().any(
            |effect| effect.payload["output"]["Notice"]["notice"]["Note"] == "UnapprovedMerge"
        )
    );
    assert_eq!(
        api.state.lock().unwrap().queue_check()["status"],
        "in_progress"
    );
}

struct RetireError(Adapter);
impl Executor for RetireError {
    fn execute(
        &mut self,
        delivery: &Delivery,
    ) -> tl3_controller::Result<tl3_controller::executor::Completion> {
        self.0.execute(delivery)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.0.reconcile(delivery)
    }
    fn retire_delivery(&mut self, _: &Delivery) -> tl3_controller::Result<()> {
        Err(tl3_controller::Error::Invalid("fixture retire error"))
    }
    fn retire(&mut self, _: &mut Store) -> tl3_controller::Result<()> {
        Err(tl3_controller::Error::Invalid(
            "fixture background retire error",
        ))
    }
}

fn retirement_does_not_block(settings: &tl3_config::Store, missing_upstream: bool) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    drain(&mut controller);
    let snapshot = controller.snapshot().unwrap();
    let mut old = delivery();
    old.key = "old-retirement".into();
    if missing_upstream {
        old.output = Out::Effect {
            node: NodeId(0),
            seq: 1,
            head: None,
            kind: EffectKind::SubmitChecks,
        };
    }
    let mut commit = tl3_store::Commit::new("inject-retirement", json!({}), &snapshot);
    commit.rows.push(tl3_store::RowChange::Put {
        table: "retiring_effects".into(),
        id: old.key.clone(),
        value: json!(old),
    });
    drop(controller);
    Store::open(settings).unwrap().commit(&commit).unwrap();
    let mut registry = if missing_upstream {
        tl3_controller::executor::Registry::default()
    } else {
        adapters::simulation(root.path()).unwrap()
    };
    if !missing_upstream {
        registry.register(
            Upstream::Mergify,
            RetireError(api.adapter(&root.path().join("mergify"), 0)),
        );
    }
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    if missing_upstream {
        assert!(controller.drive_once().unwrap());
        assert!(
            !controller
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|row| row.id == old.key && row.table == "retiring_effects")
        );
    } else {
        controller
            .observe(
                "stop-with-retire-error",
                tl3_controller::executor::Observation::Kernel(tl3_kernel::Event::Command {
                    node: NodeId(0),
                    id: 1,
                    command: tl3_kernel::Command::Stop,
                }),
            )
            .unwrap();
        drain(&mut controller);
        assert_eq!(
            controller.world().unwrap().nodes[0].phase,
            Phase::Closed(Outcome::Stopped)
        );
        assert!(
            controller
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|row| row.id == old.key && row.table == "retiring_effects")
        );
    }
}

fn marker_files(root: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension().is_some_and(|ext| {
                ext == "intent" || ext == "applied" || ext == "json" || ext == "tmp"
            })
        })
        .collect()
}

fn marker_retirement(settings: &tl3_config::Store, confirmed: bool) {
    let api = Server::new();
    api.state.lock().unwrap().lag = !confirmed;
    let root = tempfile::tempdir().unwrap();
    let records = root.path().join("mergify");
    let mut controller = queued(settings, &api, root.path(), confirmed);
    assert_eq!(marker_files(&records).len(), if confirmed { 3 } else { 2 });
    let key = controller
        .snapshot()
        .unwrap()
        .rows
        .iter()
        .find(|row| row.table == "watches" && row.value["output"]["Effect"]["kind"] == "Enqueue")
        .unwrap()
        .id
        .clone();
    drain(&mut controller);
    assert_eq!(marker_files(&records).len(), if confirmed { 3 } else { 2 });
    api.state.lock().unwrap().merged(false);
    api.state.lock().unwrap().checks = fixture("merged", "checks");
    for _ in 0..300 {
        controller.drive_once().unwrap();
        if controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "retiring_effects" && row.id == key)
        {
            break;
        }
    }
    let snapshot = controller.snapshot().unwrap();
    assert!(
        snapshot
            .rows
            .iter()
            .any(|row| row.table == "retiring_effects" && row.id == key)
    );
    assert!(
        !snapshot
            .rows
            .iter()
            .any(|row| row.table == "watches" && row.id == key)
    );
    assert!(
        !snapshot
            .pending_effects
            .iter()
            .any(|effect| effect.key == key)
    );
    // Simulate a crash partway through deleting the files, before cleanup is acknowledged.
    drop(controller);
    let intent = marker_files(&records)
        .into_iter()
        .find(|path| path.extension().unwrap() == "intent")
        .unwrap();
    std::fs::write(intent.with_extension("tmp"), b"interrupted write").unwrap();
    std::fs::remove_file(intent).unwrap();
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(Upstream::Mergify, api.adapter(&records, 0));
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    let reads = api.state.lock().unwrap().reads;
    assert!(controller.drive_once().unwrap());
    assert_eq!(api.state.lock().unwrap().reads, reads);
    assert!(marker_files(&records).is_empty());
    assert!(records.join("epoch.fence").exists());
    drain(&mut controller);
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "retiring_effects")
    );
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
    api.state.lock().unwrap().lag = false;
    assert_eq!(adapter.execute(&delivery).unwrap().ack, Ack::Confirmed);
    assert_eq!(api.state.lock().unwrap().deletes, 1);
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
    api.state.lock().unwrap().label = false;
    api.state.lock().unwrap().checks = fixture("waiting", "checks");
    drop(adapter);
    let mut adapter = api.adapter(root.path(), 2);
    assert_eq!(adapter.execute(&delivery).unwrap().ack, Ack::Confirmed);
    assert_eq!(api.state.lock().unwrap().posts, posts + 1);
    delivery.epoch = 1;
    drop(adapter);
    let mut old = api.adapter(root.path(), 1);
    assert!(old.execute(&delivery).is_err());
}
fn rate_limited_read_defers_quota(settings: &tl3_config::Store, status: u16) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Mergify,
        api.adapter(&root.path().join("mergify"), 0),
    );
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    controller
        .intake(LabelledIssue {
            delivery: "rate-limited".into(),
            owner: "owner".into(),
            repo: "sandbox".into(),
            installation: 3,
            number: 12,
            title: "change".into(),
            actor: 42,
            label: "tl3".into(),
        })
        .unwrap();
    for _ in 0..300 {
        if !controller
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
                        let mut state = api.state.lock().unwrap();
                        state.pull["head"]["sha"] = json!(delivery.head_sha);
                        state.rate_limit = Some(status);
                    }
                }
            })
            .unwrap()
        {
            break;
        }
    }
    let snapshot = controller.snapshot().unwrap();
    let allowance = snapshot
        .rows
        .iter()
        .find(|row| {
            row.table == "allowances"
                && row.value["delivery"]["output"]["Effect"]["kind"] == "Enqueue"
        })
        .expect("enqueue allowance");
    assert!(
        allowance.value["quota_ms"].as_u64().is_some(),
        "wrapped GitHub {status} must defer quota, not transport: {}",
        allowance.value
    );
    assert!(!snapshot.rows.iter().any(|row| row.table == "incidents"));
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Publishing
    );
    assert_eq!(api.state.lock().unwrap().posts, 0);
    let reads = api.state.lock().unwrap().reads;
    controller.look_now().unwrap();
    drain(&mut controller);
    assert_eq!(
        api.state.lock().unwrap().reads,
        reads,
        "quota reset must defer further reads"
    );
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "incidents")
    );
    println!("test rate_limited_read_defers_quota_{status} ... ok");
}

fn permanent_github_read(settings: &tl3_config::Store, status: u16, class: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Mergify,
        api.adapter(&root.path().join("mergify"), 0),
    );
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    controller
        .intake(LabelledIssue {
            delivery: "rate-limited".into(),
            owner: "owner".into(),
            repo: "sandbox".into(),
            installation: 3,
            number: 12,
            title: "change".into(),
            actor: 42,
            label: "tl3".into(),
        })
        .unwrap();
    for _ in 0..300 {
        if !controller
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
                        let mut state = api.state.lock().unwrap();
                        state.pull["head"]["sha"] = json!(delivery.head_sha);
                        state.http_failure = Some(status);
                    }
                }
            })
            .unwrap()
        {
            break;
        }
    }
    if status == 404 {
        let snapshot = controller.snapshot().unwrap();
        let pending = snapshot
            .pending_effects
            .iter()
            .find(|effect| effect.payload["output"]["Effect"]["kind"] == "Enqueue")
            .expect("the first enqueue 404 must be held");
        assert_eq!(pending.state, tl3_store::EffectState::Uncertain);
        assert!(!snapshot.rows.iter().any(|row| row.table == "incidents"));
        api.state.lock().unwrap().http_failure = None;
        controller.look_key(&pending.key).unwrap();
        assert_eq!(controller.world().unwrap().nodes[0].fx_failures, 1);
        api.state.lock().unwrap().http_failure = Some(status);
        drain(&mut controller);
    }
    let snapshot = controller.snapshot().unwrap();
    let incident = snapshot
        .rows
        .iter()
        .find(|row| {
            row.table == "incidents"
                && row.value["signature"] == json!({"upstream":"Mergify", "class":class})
        })
        .expect("permanent GitHub failure must raise an incident with its kind");
    assert!(
        !incident.value["summary"]
            .as_str()
            .unwrap()
            .contains("could not be reached")
    );
    assert!(
        snapshot
            .rows
            .iter()
            .filter(|row| row.table == "allowances")
            .any(|row| row.value["failure"] == class)
    );
    assert_eq!(api.state.lock().unwrap().posts, 0);
    println!("test permanent_github_read_{status} ... ok");
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--permanent-github-read") {
        let mut database = support::Database::start();
        for (status, class) in [
            (401, "permission"),
            (403, "permission"),
            (404, "not_found"),
            (410, "not_found"),
            (422, "invalid_request"),
        ] {
            permanent_github_read(
                &database.create(&format!("permanent_{status}")),
                status,
                class,
            );
        }
        database.stop();
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("--rate-limited-read") {
        let mut database = support::Database::start();
        for status in [403, 429] {
            rate_limited_read_defers_quota(&database.create(&format!("quota_{status}")), status);
        }
        database.stop();
        return;
    }
    executor_recovery();
    label_removed_before_confirmation();
    applied_marker_waits_for_person_evidence();
    println!("test executor_recovery ... ok");
    let mut database = support::Database::start();
    for status in [403, 429] {
        rate_limited_read_defers_quota(&database.create(&format!("quota_{status}")), status);
    }
    for (status, class) in [
        (401, "permission"),
        (403, "permission"),
        (404, "not_found"),
        (410, "not_found"),
        (422, "invalid_request"),
    ] {
        permanent_github_read(
            &database.create(&format!("permanent_{status}")),
            status,
            class,
        );
    }
    in_progress_confirms_enqueue(&database.create("progress"));
    late_merge_after_close(&database.create("late_webhook"), true);
    late_merge_after_close(&database.create("late_catchup"), false);
    translator_retries_after_api_failure(&database.create("translation_retry"));
    outside_merge_checks(&database.create("outside_pass"), true, true, false);
    outside_merge_checks(&database.create("outside_fail"), false, true, false);
    outside_merge_checks(&database.create("outside_no_ci"), false, false, false);
    outside_merge_checks(&database.create("stale_final_pass"), true, true, true);
    outside_merge_checks(&database.create("stale_final_fail"), false, true, true);
    for passed in [true, false] {
        for webhook in [true, false] {
            delayed_merge(
                &database.create(&format!("delayed_{passed}_{webhook}")),
                passed,
                webhook,
            );
        }
    }
    stalled_merge_progress(&database.create("stalled_progress"));
    for path in ["webhook", "catchup", "watch", "current_unapproved"] {
        unapproved_merge_does_not_wait(&database.create(&format!("unapproved_{path}")), path);
    }
    retirement_does_not_block(&database.create("retire_error"), false);
    retirement_does_not_block(&database.create("retire_missing_upstream"), true);
    marker_retirement(&database.create("retire_confirmed"), true);
    marker_retirement(&database.create("retire_uncertain"), false);
    merged(&database.create("merged_webhook"), false, true, true);
    merged(&database.create("merged_catchup"), false, false, true);
    merged(&database.create("unapproved"), true, true, true);
    merged(&database.create("missing_merge_ci"), false, true, false);
    watched_merge(&database.create("watched"));
    terminal_before_enqueue_ack(&database.create("late_enqueue_ack"));
    println!("test merged, catch-up, restart and unapproved head ... ok");
    database.restart();
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
    println!("45 Mergify scenarios passed; no skips; no live upstream calls");
}
