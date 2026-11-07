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
use support::scenario::progress as scenario_progress;
use tl3_controller::{
    Bootstrap, Boundary, Controller, LabelledIssue, Repository, adapters,
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
        fail_fast: false,
    }
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
            pem: include_str!("../../github/tests/fixtures/throwaway-key.pem").into(),
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
    posts: usize,
    pull_reads: usize,
    head_checks: Vec<Value>,
    statuses: Value,
    required: bool,
    required_app: u64,
    classic: bool,
    classic_context_only: bool,
    annotations: Value,
    body_patches: usize,
}
impl State {
    fn new() -> Self {
        let mut pull = webhook_fixture()["pull_request"].clone();
        pull["number"] = json!(100);
        pull["state"] = json!("open");
        pull["merged"] = json!(false);
        pull["merged_at"] = Value::Null;
        pull["merge_commit_sha"] = json!("dddddddddddddddddddddddddddddddddddddddd");
        Self { pull, posts: 0, pull_reads: 0, head_checks: vec![], statuses: json!([]),
            required: false, required_app: 15368, classic: false,
            classic_context_only: false, annotations: json!([]), body_patches: 0 }
    }
    fn merged(&mut self, unapproved: bool) {
        self.pull["state"] = json!("closed");
        self.pull["merged"] = json!(true);
        self.pull["merged_at"] = json!("2026-10-30T01:00:00Z");
        if unapproved { self.pull["head"]["sha"] = json!("b".repeat(40)); }
    }
}
#[derive(Clone)]
struct Api(Arc<Mutex<State>>);
impl support::github::Routes for State {
    fn pulls(&self) -> Value { json!([self.pull]) }
    fn checks(&self, _sha: &str) -> Value { json!(self.head_checks) }
    fn pull_mut(&mut self) -> Option<&mut Value> { Some(&mut self.pull) }
    fn pull_patched(&mut self, body: &Value) {
        self.body_patches += usize::from(body.get("body").is_some());
    }
}
impl Respond for Api {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.0.lock().unwrap();
        let normalized = request.url.path()
            .replace("/repos/RobTand/prismabuild", "/repos/owner/sandbox")
            .replace("/repos/RobTand/trustladderiii", "/repos/owner/sandbox");
        let path = normalized.as_str();
        let method = request.method.as_str();
        if let Some(response) = support::github::access_token(path, 3, "disposable") {
            return response;
        }
        if let Some(response) = support::github::respond(request, path, "/repos/owner/sandbox", &mut *state) {
            return response;
        }
        match (method, path) {
            ("GET", "/repos/owner/sandbox/branches/main" | "/repos/owner/sandbox/branches/release") => ResponseTemplate::new(200).set_body_json(
                if state.classic { json!({"protection":{"required_status_checks":{
                    "contexts":["Sandbox tests"], "checks":if state.classic_context_only {json!([])} else {json!([{"context":"Sandbox tests","app_id":15368}])}}}}) }
                else { json!({"protected":false}) }),
            ("GET", "/repos/owner/sandbox/rules/branches/main" | "/repos/owner/sandbox/rules/branches/release") => ResponseTemplate::new(200).set_body_json(
                if state.required { json!([{"type":"required_status_checks","parameters":{"required_status_checks":[{"context":"Sandbox tests","integration_id":state.required_app}]}}]) } else { json!([]) }),
            ("GET", path) if path.ends_with("/statuses") => ResponseTemplate::new(200).set_body_json(state.statuses.clone()),
            ("GET", "/repos/owner/sandbox/actions/runs") => ResponseTemplate::new(200).set_body_json(json!({"workflow_runs":[]})),
            ("GET", "/repos/owner/sandbox/pulls/100/files") => ResponseTemplate::new(200).set_body_json(json!([])),
            ("GET", "/repos/owner/sandbox/issues/12") => ResponseTemplate::new(200).set_body_json(json!({
                "number":12, "title":"change", "state":"open", "updated_at":"2026-10-31T16:00:00Z", "labels":[],
                "body":"Add the lane.\n\n## PR rules\n\n- The PR body has exactly one `Change type: feature` line.\n"})),
            ("GET", "/repos/owner/sandbox/issues/100") => ResponseTemplate::new(200).set_body_json(json!({
                "number":100, "title":"change", "state":state.pull["state"], "updated_at":state.pull["updated_at"],
                "labels":[],"pull_request":{"url":"local"}})),
            ("GET", "/repos/owner/sandbox/issues/100/events" | "/repos/owner/sandbox/issues/events" |
                "/repos/owner/sandbox/issues" | "/repos/owner/sandbox/issues/comments") => ResponseTemplate::new(200).set_body_json(json!([])),
            ("GET", path) if path.starts_with("/repos/owner/sandbox/pulls/") => ResponseTemplate::new(404),
            _ => panic!("unexpected Direct fixture request {method} {path}"),
        }
    }
}
#[derive(Clone)]
struct Clock(std::rc::Rc<std::cell::Cell<std::time::SystemTime>>);
impl tl3_controller::clock::Clock for Clock {
    fn now(&self) -> std::time::SystemTime { self.0.get() }
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
}
fn drain(controller: &mut Controller) {
    scenario_progress("direct: drain ingress");
    ingress::drain(controller).unwrap();
    for _ in 0..300 {
        scenario_progress("direct: controller step");
        if !controller.drive_once().unwrap() {
            return;
        }
    }
    panic!("controller did not settle");
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
    assert_eq!(
        post(address, &first, event, body),
        202,
        "event {event}, action {}",
        body["action"]
    );
    assert_eq!(
        post(address, &first, event, body),
        202,
        "event {event}, action {}",
        body["action"]
    );
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

include!("direct_merge/scenarios.rs");
