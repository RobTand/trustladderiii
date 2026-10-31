#[path = "../../store/tests/support/mod.rs"]
mod support;
mod worktree_fixture;

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use tl3_controller::executor::Output as Out;
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository,
    adapters::{
        self,
        github::APPROVAL_CHECK,
        prismabuild::{Check, CheckRun, Kind},
        workers::{
            Supervisor, WorkerOutputSource,
            writer::{FakeWriter, TextDraft, WriteRequest, Writer, WriterReply},
        },
        worktrees::Real,
    },
    cli::Input,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation, Upstream},
};
use tl3_github::{AppConfig, ClientOptions, GitHub, message::Policy};
use tl3_kernel::{EffectKind, NodeId, Outcome, Phase, Role};
use tl3_prismabuild::{Demand, Options, PrismaBuild, Tool};
use tl3_store::{Commit, RowChange, Store};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};
use worktree_fixture::{Fixture, commit, git};

fn repo() -> Repository {
    worktree_fixture::identity()
}
fn operations() -> tl3_config::OperationsIssue {
    tl3_config::OperationsIssue {
        owner: repo().owner,
        name: "operations".into(),
        installation_id: repo().installation,
        number: 81,
    }
}
fn bootstrap() -> Bootstrap {
    Bootstrap {
        repos: vec![repo()],
        rob: 1,
        slots: 1,
        pool: 1,
        operations: Some(operations()),
    }
}
fn check_plan(f: &Fixture, base: String) -> CheckRun {
    CheckRun {
        repository: f.repo.clone(),
        base_sha: base,

        reviewed_sha: None,
        reproducing_unavailable: None,
        plan: vec![Check {
            name: "value".into(),
            kind: Kind::Demonstrating,
            tests: vec!["test_value.py::test_value".into()],
        }],
    }
}
fn base(f: &Fixture, d: &Delivery) -> String {
    git(
        &f.repo,
        &[
            "rev-parse",
            &format!("refs/tl3/bases/{}", Real::branch(d).unwrap().as_str()),
        ],
    )
}
fn check_observation(f: &Fixture, d: &Delivery, head: String) -> Observation {
    Observation::CheckPlan {
        node: d.node(),
        sha: head,
        run: check_plan(f, base(f, d)),
    }
}
struct Workers {
    real: Supervisor,
    fake: adapters::workers::Fake,
    fixture: Arc<Fixture>,
}
impl Executor for Workers {
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        if matches!(
            d.output,
            Out::Effect {
                kind: EffectKind::StartRun(Role::Review) | EffectKind::StopRun,
                ..
            }
        ) {
            return self.real.execute(d);
        }
        let mut c = self.fake.execute(d)?;
        for o in &mut c.observations {
            match o {
                Observation::Plan { paths, .. } => *paths = vec!["value.py".into()],
                Observation::Made { sha, .. } => {
                    let checkout = self.fixture.adapter().checkout(d);
                    if fs::read_to_string(checkout.join("value.py")).unwrap()
                        == "def value(): return 2\n"
                    {
                        *sha = commit(&checkout, "value.py", "def value():\n    return 2\n");
                    } else {
                        *sha = commit(&checkout, "value.py", "def value(): return 2\n");
                    }
                }
                _ => {}
            }
        }
        if let Some(Observation::Made { sha, .. }) = c.observations.first() {
            c.observations
                .insert(0, check_observation(&self.fixture, d, sha.clone()));
        }
        Ok(c)
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        if matches!(
            d.output,
            Out::Effect {
                kind: EffectKind::StartRun(Role::Review) | EffectKind::StopRun,
                ..
            }
        ) {
            self.real.reconcile(d)
        } else {
            self.fake.reconcile(d)
        }
    }
    fn retire(&mut self, s: &mut Store) -> tl3_controller::Result<()> {
        self.real.retire(s)
    }
}

struct State {
    pull: Option<Value>,
    checks: Vec<Value>,
    comments: Vec<Value>,
    operations_comments: Vec<Value>,
    labels: bool,
    closed: bool,
    fail_reads: bool,
    lose_response: Option<&'static str>,
    opens: usize,
    reopens: usize,
    closes: usize,
    operations: Vec<String>,
    queue_fixture: &'static str,
}
impl Default for State {
    fn default() -> Self {
        Self {
            pull: None,
            checks: vec![],
            comments: vec![],
            operations_comments: vec![],
            labels: false,
            closed: false,
            fail_reads: false,
            lose_response: None,
            opens: 0,
            reopens: 0,
            closes: 0,
            operations: vec![],
            queue_fixture: "running",
        }
    }
}
fn recorded_check(name: &str, head: &Value) -> Value {
    let checks: Vec<Value> = serde_json::from_slice(
        &fs::read(format!(
            "{}/../mergify/tests/fixtures/{name}/checks.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    let mut check = checks
        .into_iter()
        .find(|c| c["name"] == tl3_mergify::QUEUE_CHECK)
        .unwrap();
    check["head_sha"] = head.clone();
    check
}
#[derive(Clone)]
struct Api {
    state: Arc<Mutex<State>>,
    fixture: Arc<Fixture>,
}
struct PublishingWriter;
impl Writer for PublishingWriter {
    fn draft(&mut self, key: &str, input: &WriteRequest) -> tl3_controller::Result<WriterReply> {
        if input.facts["change"]
            .as_str()
            .is_some_and(|change| change.contains("clear incident "))
        {
            assert_eq!(input.facts["issue_title"], "Fix the value");
            assert!(
                input.facts["request"]
                    .as_str()
                    .unwrap()
                    .contains("https://github.com/RobTand/tl3-sandbox/issues/42")
            );
            return Ok(WriterReply::Text(TextDraft {
                text: "You asked for \"Fix the value\".\n\nThe change merged without the required approval. Please check the change.".into(),
            }));
        }
        FakeWriter.draft(key, input)
    }
}
impl Respond for Api {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        let mut s = self.state.lock().unwrap();
        let method = r.method.as_str();
        let path = r.url.path();
        if path == "/app/installations/1/access_tokens" {
            return ResponseTemplate::new(201)
                .set_body_json(json!({"token":"disposable", "expires_at":"2099-01-01T00:00:00Z"}));
        }
        if s.fail_reads && method == "GET" {
            return ResponseTemplate::new(503);
        }
        if path == "/repos/RobTand/operations/issues/81/comments" {
            return match method {
                "GET" => ResponseTemplate::new(200).set_body_json(json!(s.operations_comments)),
                "POST" => {
                    let body: Value = serde_json::from_slice(&r.body).unwrap();
                    let comment = json!({"id":s.operations_comments.len() as u64+1,"body":body["body"],"user":{"id":99,"login":"factory[bot]"},"created_at":"2026-10-30T00:00:00Z","updated_at":"2026-10-30T00:00:00Z","issue_url":"https://api.github.com/repos/RobTand/operations/issues/81"});
                    s.operations_comments.push(comment.clone());
                    ResponseTemplate::new(201).set_body_json(comment)
                }
                _ => panic!("unexpected operations request: {method} {path}"),
            };
        }
        let prefix = "/repos/RobTand/tl3-sandbox";
        let path = path.strip_prefix(prefix).unwrap_or(path);
        let body: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
        let response = match (method, path) {
            ("GET", "/pulls") => json!(s.pull.iter().collect::<Vec<_>>()),
            ("POST", "/pulls") => {
                s.opens += 1;
                s.operations.push("open".into());
                let head = git(
                    &self.fixture.remote,
                    &["rev-parse", body["head"].as_str().unwrap()],
                );
                let base = git(&self.fixture.remote, &["rev-parse", "main"]);
                s.pull = Some(
                    json!({"number":100,"user":{"id":99,"login":"factory[bot]"},"state":"open","merged_at":null,"merge_commit_sha":null,
                    "head":{"ref":body["head"],"sha":head,"repo":{"id":1,"name":"tl3-sandbox","owner":{"id":1,"login":"RobTand"}}},
                    "base":{"ref":"main","sha":base,"repo":null},"updated_at":"2026-10-30T00:00:00Z"}),
                );
                s.pull.clone().unwrap()
            }
            ("GET", "/pulls/100") => {
                if let Some(pull) = &mut s.pull
                    && pull["state"] == "open"
                {
                    pull["head"]["sha"] = json!(git(
                        &self.fixture.remote,
                        &["rev-parse", pull["head"]["ref"].as_str().unwrap()]
                    ));
                }
                s.pull.clone().unwrap()
            }
            ("PATCH", "/pulls/100") => {
                if body["state"] == "open" {
                    s.reopens += 1;
                } else {
                    s.closes += 1;
                }
                s.pull.as_mut().unwrap()["state"] = body["state"].clone();
                s.pull.clone().unwrap()
            }
            ("GET", "/issues/42" | "/issues/100") => {
                json!({"number":42,"title":"Fix the value", "body":"Return two.","state":if s.closed {"closed"} else {"open"},"updated_at":"2026-10-30T00:00:00Z","labels":if s.labels {json!([{"name":"tl3-queue"}])} else {json!([])},"pull_request":null})
            }
            ("PATCH", "/issues/42") => {
                s.closed = true;
                s.operations.push("close-issue".into());
                json!({"number":42,"title":"Fix the value","body":"Return two.","state":"closed","updated_at":"2026-10-30T00:00:00Z"})
            }
            ("GET", "/issues/42/comments") => json!(s.comments),
            ("POST", "/issues/42/comments") => {
                if body["body"]
                    .as_str()
                    .unwrap()
                    .contains("Your change was merged.")
                {
                    assert!(
                        git(&self.fixture.remote, &["for-each-ref", "refs/heads/tl3"]).is_empty()
                    );
                }
                s.operations.push("comment".into());
                let c = json!({"id":s.comments.len() as u64+1,"body":body["body"],"user":{"id":99,"login":"factory[bot]"},"created_at":"2026-10-30T00:00:00Z","updated_at":"2026-10-30T00:00:00Z","issue_url":"https://api.github.com/repos/RobTand/tl3-sandbox/issues/42"});
                s.comments.push(c.clone());
                c
            }
            ("GET", "/issues/100/events") => json!([]),
            ("POST", "/issues/100/labels") => {
                let head = &s.pull.as_ref().unwrap()["head"]["sha"];
                assert!(s.checks.iter().any(|c| c["name"] == APPROVAL_CHECK
                    && c["head_sha"] == *head
                    && c["app"]["id"] == 123));
                s.operations.push("enqueue".into());
                s.labels = true;
                json!([{"name":"tl3-queue"}])
            }
            ("DELETE", "/issues/100/labels/tl3-queue") => {
                s.labels = false;
                json!([])
            }
            ("GET", path) if path.ends_with("/check-runs") => {
                let sha = path.split('/').nth(2).unwrap();
                let mut checks: Vec<_> = s
                    .checks
                    .iter()
                    .filter(|c| c["head_sha"] == sha)
                    .cloned()
                    .collect();
                if s.labels && s.pull.as_ref().is_some_and(|p| p["head"]["sha"] == sha) {
                    checks.push(recorded_check(s.queue_fixture, &json!(sha)));
                }
                json!({"check_runs":checks})
            }
            ("POST", "/check-runs") => {
                let mut c = body.clone();
                c["id"] = json!(s.checks.len() + 1);
                c["app"] = json!({"id":123,"slug":"factory"});
                s.operations.push("approve".into());
                s.checks.push(c.clone());
                c
            }
            _ => panic!("unexpected mock request: {method} {path}"),
        };
        if method != "GET" && s.lose_response == Some(path) {
            s.lose_response = None;
            return ResponseTemplate::new(503);
        }
        ResponseTemplate::new(200).set_body_json(response)
    }
}
struct Server {
    runtime: tokio::runtime::Runtime,
    server: MockServer,
    api: Api,
}
impl Server {
    fn new(fixture: Arc<Fixture>) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let api = Api {
            state: Arc::new(Mutex::new(State::default())),
            fixture,
        };
        let server = runtime.block_on(async {
            let server = MockServer::start().await;
            Mock::given(|_: &Request| true)
                .respond_with(api.clone())
                .mount(&server)
                .await;
            server
        });
        Self {
            runtime,
            server,
            api,
        }
    }
    fn clients(&self) -> BTreeMap<u64, GitHub> {
        BTreeMap::from([(
            1,
            GitHub::new(
                AppConfig {
                    id: 123,
                    slug: "factory".into(),
                    pem: include_str!("../../mergify/tests/fixtures/throwaway-key.pem").into(),
                    webhook_secret: "fixture".into(),
                },
                1,
                ClientOptions {
                    api_url: format!("{}/", self.server.uri()).parse().unwrap(),
                    max_retries: 0,
                    mutation_interval: Duration::ZERO,
                    max_wait: Duration::ZERO,
                    ..ClientOptions::default()
                },
            )
            .unwrap(),
        )])
    }
    fn adapter(&self, root: &Path, epoch: i64) -> adapters::github::Adapter {
        adapters::github::Adapter::new(
            self.clients(),
            PublishingWriter,
            epoch,
            Policy::default(),
            root,
            Some(operations()),
        )
        .unwrap()
    }
}
fn pb(root: &Path) -> adapters::prismabuild::Adapter {
    let queue = root.join("queue");
    for state in ["ready", "claimed", "done", "failed", "withdrawn"] {
        fs::create_dir_all(queue.join(state)).unwrap();
    }
    fs::write(queue.join("mode"), "done").unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tool = Tool {
        program: "python3".into(),
        args: vec![
            manifest.join("tests/fixtures/pb.py").into(),
            queue.clone().into(),
            manifest.join("../prismabuild/tests/fixtures/queue").into(),
        ],
    };
    adapters::prismabuild::Adapter::new(
        PrismaBuild::new(Options {
            queue_root: queue,
            tool: tool.clone(),
            bell_url: "http://127.0.0.1:9/".into(),
        })
        .unwrap(),
        adapters::prismabuild::Settings {
            records: root.join("pb-records"),
            harness: manifest.join("../../harness/src/tl3_harness"),
            python: vec!["python3.14".into()],
            image: format!("sha256:{}", "a".repeat(64)),
            demand: Demand {
                cpus: 1.try_into().unwrap(),
                mem_gb: 2,
                gpu: false,
            },
            quiet: Duration::from_secs(120),
            priority: -10,
            result_tool: tool,
        },
    )
    .unwrap()
}
fn document(store: &mut Store, table: &str, value: Value) {
    let snapshot = store.load().unwrap();
    let mut c = Commit::new(
        format!("fixture-{table}"),
        json!({"fixture":table}),
        &snapshot,
    );
    c.rows.push(RowChange::Put {
        table: table.into(),
        id: "0".into(),
        value,
    });
    store.commit(&c).unwrap();
}
fn prepare_workers() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../workers")
        .canonicalize()
        .unwrap();
    assert!(
        Command::new(env::var_os("TL3_TEST_UV").unwrap_or("uv".into()))
            .args(["sync", "--locked", "--python", "3.14+gil", "--project"])
            .arg(&root)
            .status()
            .expect("worker tests require uv 0.10.10: set TL3_TEST_UV or install uv on PATH")
            .success()
    );
    root.join(".venv/bin/python")
}
fn drain(c: &mut Controller, source: &mut WorkerOutputSource, store: &mut Store, expected: Phase) {
    let (sender, receiver) = mpsc::channel();
    let start = Instant::now();
    loop {
        let mut changed = false;
        for _ in 0..500 {
            if !c.drive_once().unwrap() {
                break;
            }
            changed = true;
        }
        source.poll_once(store, &sender).unwrap();
        for input in receiver.try_iter() {
            if let Input::Observation { key, observation } = input {
                c.observe(&key, observation).unwrap();
                changed = true;
            }
        }
        if !changed && c.world().unwrap().nodes[0].phase == expected {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "publishing did not settle: {:?}",
            c.world().unwrap().nodes[0]
        );
        if !changed {
            thread::sleep(Duration::from_millis(10));
        }
    }
}
fn webhook(c: &mut Controller, server: &Server, action: &str) {
    let mut body: Value = serde_json::from_str(include_str!(
        "../../github/tests/fixtures/pull-request-merged.json"
    ))
    .unwrap();
    body["action"] = json!(action);
    body["repository"] = json!({"id":1,"name":"tl3-sandbox","owner":{"id":1,"login":"RobTand"}});
    body["installation"] = json!({"id":1});
    body["sender"] = json!({"id":1,"login":"rob"});
    body["pull_request"] = server.api.state.lock().unwrap().pull.clone().unwrap();
    if action == "synchronize" {
        body["before"] = body["pull_request"]["head"]["sha"].clone();
        let branch = body["pull_request"]["head"]["ref"].as_str().unwrap();
        let after = git(&server.api.fixture.remote, &["rev-parse", branch]);
        body["after"] = json!(after);
        body["pull_request"]["head"]["sha"] = json!(after);
        server.api.state.lock().unwrap().pull = Some(body["pull_request"].clone());
    }
    body["pull_request"]["merged"] = json!(action == "closed");
    let event = tl3_github::webhook::parse(
        "publishing",
        "pull_request",
        &serde_json::to_vec(&body).unwrap(),
    )
    .unwrap()
    .event
    .unwrap();
    let inputs = adapters::mergify::Translator::new(server.clients())
        .unwrap()
        .webhook(&event, &c.snapshot().unwrap(), &bootstrap().repos)
        .unwrap();
    for input in inputs {
        tl3_controller::ingress::apply(c, input).unwrap();
    }
}
fn patch_lines(diff: &Value) -> Vec<&str> {
    diff.as_str()
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("index "))
        .collect()
}
fn lifecycle(settings: &tl3_config::Store, python: &Path, mode: &str) {
    let root = tempfile::tempdir().unwrap();
    let f = Arc::new(Fixture::create(root.path()));
    commit(&f.source, "value.py", "def value(): return 1\n");
    commit(
        &f.source,
        "pyproject.toml",
        "[tool.tl3.harness]\ncommand = ['python', '-m', 'pytest']\ntest-files = ['test_*.py']\n",
    );
    commit(
        &f.source,
        "test_value.py",
        "from value import value\ndef test_value(): assert value() == 2\n",
    );
    git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
    let server = Server::new(f.clone());
    let mut config = tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
    config.store = settings.clone();
    if mode.starts_with("out-of-room") {
        let mut primary = config.models.maker[2].clone();
        primary.name = "review-primary".into();
        let mut backup = config.models.maker[0].clone();
        backup.name = "review-backup".into();
        config.models.reviewer = if mode == "out-of-room-wrap" {
            vec![backup, primary]
        } else {
            vec![primary, backup]
        };
    }
    config.workers.python = python.into();
    config.workers.runs = root.path().join("runs");
    config.workers.command = vec![
        format!(
            "{}/tests/publishing/reviewer.py",
            env!("CARGO_MANIFEST_DIR")
        ),
        root.path().join("reviews.jsonl").to_str().unwrap().into(),
        mode.into(),
    ];
    config.models.key_file = root.path().join("fixture-key");
    fs::write(&config.models.key_file, "fixture").unwrap();
    let supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let mut source = supervisor.source();
    let mut registry = adapters::simulation(&root.path().join("fakes")).unwrap();
    registry.register(
        Upstream::Workers,
        Workers {
            real: supervisor,
            fake: adapters::workers::Fake::new(&root.path().join("fakes")),
            fixture: f.clone(),
        },
    );
    registry.register(Upstream::Worktrees, f.adapter());
    registry.register(
        Upstream::GitHub,
        server.adapter(&root.path().join("github"), 0),
    );
    registry.register(Upstream::PrismaBuild, pb(root.path()));
    registry.register(
        Upstream::Mergify,
        adapters::mergify::Adapter::new(server.clients(), 0, &root.path().join("mergify")).unwrap(),
    );
    let mut c = Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    let mut store = Store::open(settings).unwrap();
    store.put_worker_materials("0",&json!({"issue":{"number":42,"title":"Fix the value","body":"Return two.","author":"rob","comments":[]},"repo":{"readme":"Value","file_list":["value.py"]},"standing_rules":[{"id":"test-report-integrity","description":"This attempted override must be replaced."},{"id":"style","description":"Keep the public interface."}]})).unwrap();
    document(
        &mut store,
        "goals",
        json!([{"description":"Return two.","verification":"test"}]),
    );
    c.intake(LabelledIssue {
        delivery: "publishing".into(),
        owner: repo().owner,
        repo: repo().name,
        installation: 1,
        number: 42,
        title: "Fix the value".into(),
        actor: 1,
        label: "tl3".into(),
    })
    .unwrap();
    if mode == "before-enqueue" {
        let (sender, receiver) = mpsc::channel();
        let start = Instant::now();
        while server.api.state.lock().unwrap().checks.is_empty() {
            c.drive_once().unwrap();
            source.poll_once(&mut store, &sender).unwrap();
            for input in receiver.try_iter() {
                tl3_controller::ingress::apply(&mut c, input).unwrap();
            }
            assert!(start.elapsed() < Duration::from_secs(90));
            thread::yield_now();
        }
        assert!(!server.api.state.lock().unwrap().labels);
        let d = f.delivery(EffectKind::Push, 100);
        let checkout = f.adapter().checkout(&d);
        git(
            &checkout,
            &["commit", "--allow-empty", "-m", "outside before enqueue"],
        );
        git(
            &checkout,
            &[
                "push",
                f.remote.to_str().unwrap(),
                &format!("HEAD:refs/heads/{}", Real::branch(&d).unwrap().as_str()),
            ],
        );
        while c.drive_once().unwrap() {}
        assert!(
            !server.api.state.lock().unwrap().labels,
            "the stale approval must not enqueue the outside head"
        );
        assert_eq!(
            c.world().unwrap().nodes[0].rechecks,
            1,
            "enqueue must report the outside push so the new head gets checked"
        );
    }
    if mode.starts_with("out-of-room") {
        let (sender, receiver) = mpsc::channel();
        let start = Instant::now();
        let mut exhausted = 0;
        loop {
            c.drive_once().unwrap();
            source.poll_once(&mut store, &sender).unwrap();
            for input in receiver.try_iter() {
                if let Input::Observation { key, observation } = input {
                    let review_reply = matches!(&observation, Observation::WorkerReply { role, reply, .. } if role == "reviewer" && reply["verdict"] == "out_of_room");
                    // The supervisor must accept the exhaustion reply, not report a crash.
                    assert!(
                        !matches!(
                            &observation,
                            Observation::Kernel(tl3_kernel::Event::Reply {
                                reply: tl3_kernel::Reply::RunFailed,
                                ..
                            })
                        ),
                        "Q44 exhaustion must be a reply"
                    );
                    c.observe(&key, observation).unwrap();
                    if review_reply {
                        exhausted += 1;
                        let n = c.world().unwrap().nodes[0];
                        assert_eq!(n.run_failures, 0);
                        if exhausted == 2 {
                            assert_eq!(n.phase, Phase::Planning { must_split: true });
                            assert!(!n.holds);
                            assert_eq!(n.credit.balance(), 3);
                            let reviews: Vec<Value> =
                                fs::read_to_string(root.path().join("reviews.jsonl"))
                                    .unwrap()
                                    .lines()
                                    .map(|line| serde_json::from_str(line).unwrap())
                                    .collect();
                            assert_eq!(
                                reviews.len(),
                                if mode == "out-of-room-fix" { 3 } else { 2 }
                            );
                            let first = &reviews[reviews.len() - 2];
                            let fresh = &reviews[reviews.len() - 1];
                            assert_eq!(fresh["resume_session"], false);
                            assert_ne!(first["session_file"], fresh["session_file"]);
                            assert_ne!(
                                first["operation"]["generation"],
                                fresh["operation"]["generation"]
                            );
                            assert_eq!(first["model"]["name"], "review-primary");
                            assert_eq!(fresh["model"]["name"], "review-backup");
                            assert_eq!(
                                fresh["inputs"]["base_sha"],
                                reviews[0]["inputs"]["base_sha"]
                            );
                            return;
                        }
                        assert_eq!(n.phase, Phase::Reviewing);
                        assert_eq!(n.rechecks, 1);
                    }
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(90),
                "Q44 did not settle"
            );
            thread::yield_now();
        }
    }
    drain(&mut c, &mut source, &mut store, Phase::Queued);
    let first_head = server.api.state.lock().unwrap().pull.as_ref().unwrap()["head"]["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    let credit = c.world().unwrap().nodes[0].credit.balance();
    let d = f.delivery(EffectKind::Push, 100);
    if matches!(mode, "push" | "reports-latest") {
        let checkout = f.adapter().checkout(&d);
        git(
            &checkout,
            &["commit", "--allow-empty", "-m", "outside commit"],
        );
        git(
            &checkout,
            &[
                "push",
                f.remote.to_str().unwrap(),
                &format!("HEAD:refs/heads/{}", Real::branch(&d).unwrap().as_str()),
            ],
        );

        webhook(&mut c, &server, "synchronize");
        drain(&mut c, &mut source, &mut store, Phase::Queued);
    } else if matches!(mode, "rebase" | "round-one-rebase") {
        f.advance_main(false);
        server.api.state.lock().unwrap().queue_fixture = "conflict";
        let new_target = git(&f.remote, &["rev-parse", "main"]);
        loop {
            c.drive_once().unwrap();
            let n = c.world().unwrap().nodes[0];
            if n.phase == Phase::Checking {
                let head = f.head(&d);
                assert_ne!(head, first_head);
                let plan = c
                    .snapshot()
                    .unwrap()
                    .rows
                    .into_iter()
                    .find(|row| row.table == "check_plans" && row.id == format!("0:{head}"))
                    .unwrap();
                assert_eq!(plan.value["base_sha"], new_target);
                break;
            }
        }
        server.api.state.lock().unwrap().queue_fixture = "running";
        drain(&mut c, &mut source, &mut store, Phase::Queued);
    }
    let reviews: Vec<Value> = fs::read_to_string(root.path().join("reviews.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        reviews.len(),
        if matches!(mode, "new-attempt" | "review-retry" | "round-one-rebase") {
            3
        } else if matches!(
            mode,
            "push" | "rebase" | "changes" | "before-enqueue" | "reports-latest"
        ) {
            2
        } else {
            1
        }
    );
    if mode == "round-one-rebase" {
        assert_eq!(c.world().unwrap().nodes[0].round, 1);
        assert_eq!(reviews[2]["resume_session"], true);
        assert_eq!(reviews[1]["session_file"], reviews[2]["session_file"]);
        assert_eq!(
            reviews[2]["inputs"]["base_sha"], reviews[2]["inputs"]["check_base_sha"],
            "a rebased review must start at the leaf's new check base"
        );
        assert_eq!(
            patch_lines(&reviews[2]["inputs"]["diff"]),
            [
                "diff --git a/value.py b/value.py",
                "--- a/value.py",
                "+++ b/value.py",
                "@@ -1 +1,2 @@",
                "-def value(): return 1",
                "+def value():",
                "+    return 2",
            ],
            "rebased round 1 must contain only the leaf's own change"
        );
    }
    if matches!(
        mode,
        "changes" | "new-attempt" | "review-retry" | "round-one-rebase"
    ) {
        assert_eq!(reviews[0]["resume_session"], false);
        assert_eq!(reviews[1]["resume_session"], true);
        assert_eq!(reviews[0]["session_file"], reviews[1]["session_file"]);
        assert_eq!(
            reviews[1]["inputs"]["base_sha"],
            reviews[0]["inputs"]["head_sha"]
        );
        assert_eq!(
            patch_lines(&reviews[1]["inputs"]["diff"]),
            [
                "diff --git a/value.py b/value.py",
                "--- a/value.py",
                "+++ b/value.py",
                "@@ -1 +1,2 @@",
                "-def value(): return 2",
                "+def value():",
                "+    return 2",
            ],
            "round 1 must contain exactly the maker's fix"
        );
        assert!(Path::new(reviews[0]["session_file"].as_str().unwrap()).is_file());
    }
    if mode == "new-attempt" {
        assert_eq!(reviews[2]["resume_session"], false);
        assert_ne!(reviews[1]["session_file"], reviews[2]["session_file"]);
        assert_eq!(c.world().unwrap().nodes[0].attempt, 2);
    }
    if mode == "review-retry" {
        assert_eq!(reviews[1]["session_file"], reviews[2]["session_file"]);
        assert_eq!(reviews[1]["inputs"], reviews[2]["inputs"]);
        assert_ne!(
            reviews[1]["operation"]["generation"],
            reviews[2]["operation"]["generation"]
        );
    }
    for (index, review) in reviews.iter().enumerate() {
        assert_eq!(
            review["inputs"]["check_base_sha"],
            if review["resume_session"] == true && !(mode == "round-one-rebase" && index == 2) {
                reviews[0]["inputs"]["base_sha"].clone()
            } else {
                review["inputs"]["base_sha"].clone()
            },
            "check outcomes must name their own comparison base"
        );
        assert_ne!(review["model"]["name"], config.models.maker[0].name);
        assert!(
            review["inputs"]["standing_rules"][1]["description"]
                .as_str()
                .unwrap()
                .contains("Protect test reports")
        );
    }
    if matches!(
        mode,
        "push" | "rebase" | "round-one-rebase" | "before-enqueue" | "reports-latest"
    ) {
        assert_ne!(
            reviews[0]["inputs"]["head_sha"],
            reviews[1]["inputs"]["head_sha"]
        );
        assert_eq!(c.world().unwrap().nodes[0].credit.balance(), credit);
        assert_eq!(c.world().unwrap().nodes[0].rechecks, 1);
    }
    if mode == "reports-latest" {
        let snapshot = c.snapshot().unwrap();
        let reports: Vec<_> = snapshot
            .rows
            .iter()
            .filter(|r| r.table == "check_reports")
            .collect();
        assert_eq!(
            reports.len(),
            1,
            "two heads must leave one check report per node"
        );
        assert_eq!(reports[0].value["results"], reviews[1]["inputs"]["checks"]);
        assert!(
            !snapshot.rows.iter().any(|r| r.table == "review_reports"),
            "unused review reports must not accumulate"
        );
    }
    let mut state = server.api.state.lock().unwrap();
    if mode == "unapproved" {
        state.pull.as_mut().unwrap()["head"]["sha"] = json!("f".repeat(40));
    }
    state.pull.as_mut().unwrap()["state"] = json!("closed");
    state.pull.as_mut().unwrap()["merged_at"] = json!("2026-10-30T01:00:00Z");
    state.pull.as_mut().unwrap()["merge_commit_sha"] = json!("e".repeat(40));
    state.queue_fixture = "merged";
    drop(state);
    webhook(&mut c, &server, "closed");
    drain(
        &mut c,
        &mut source,
        &mut store,
        Phase::Closed(Outcome::Merged),
    );
    if mode == "reports-closed" {
        assert!(
            !c.snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "check_reports"),
            "a closed node must leave no check reports"
        );
    }
    assert!(
        !config.workers.runs.join("sessions/0").exists(),
        "closed node must retire maker and reviewer sessions"
    );
    assert!(!f.adapter().checkout(&d).exists());
    assert!(git(&f.remote, &["for-each-ref", "refs/heads/tl3"]).is_empty());
    assert!(!root.path().join("pb-records/0").exists());
    assert!(store.worker_progress(None).unwrap().is_empty());
    let state = server.api.state.lock().unwrap();
    assert!(state.closed);
    assert_eq!(state.opens, 1);
    assert_eq!(
        state.checks.len(),
        if mode == "round-one-rebase" {
            2
        } else if matches!(mode, "changes" | "new-attempt" | "review-retry") {
            1
        } else {
            reviews.len()
        }
    );
    let at = |name: &str| state.operations.iter().position(|op| op == name).unwrap();
    assert!(at("approve") < at("open") && at("open") < at("enqueue"));
    assert!(state.comments.iter().any(|c| {
        c["body"]
            .as_str()
            .unwrap()
            .contains("Your change was merged.")
    }));
    assert_eq!(
        state.operations_comments.len(),
        usize::from(mode == "unapproved"),
        "only an unapproved merge must post an operations incident"
    );
    if mode == "unapproved" {
        assert!(
            state.operations_comments[0]["body"]
                .as_str()
                .unwrap()
                .contains("without the required approval")
        );
        assert!(state.comments.iter().any(|c| {
            c["body"]
                .as_str()
                .unwrap()
                .contains("without the required approval")
        }));
        assert!(store.effects().unwrap().iter().any(|e| {
            e.state == tl3_store::EffectState::Confirmed
                && serde_json::from_value::<Delivery>(e.payload.clone()).is_ok_and(|d| {
                    d.repository.name == operations().name
                        && d.issue == operations().number
                        && d.note
                            .as_deref()
                            .unwrap()
                            .contains("https://github.com/RobTand/tl3-sandbox/issues/42")
                        && matches!(
                            d.output,
                            Out::Notice {
                                notice: tl3_kernel::Notice::Incident { .. },
                                ..
                            }
                        )
                })
        }));
    }
    let calls = fs::read_to_string(root.path().join("queue/calls.jsonl")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.contains("--seal-only"))
            .count(),
        if mode == "review-retry" {
            2
        } else {
            reviews.len()
        }
    );
    drop(state);
    drop(c);
    drop(server.runtime);
}
fn publication_recovery() {
    let root = tempfile::tempdir().unwrap();
    let f = Arc::new(Fixture::create(root.path()));
    let prepare = f.delivery(EffectKind::PrepareWorktree, 1);
    f.adapter().execute(&prepare).unwrap();
    let sha = commit(&f.adapter().checkout(&prepare), "leaf.txt", "change\n");
    let mut d = f.delivery(EffectKind::Push, 2);
    d.head_sha = Some(sha.clone());
    f.adapter().execute(&d).unwrap();
    let server = Server::new(f);
    let records = root.path().join("github");
    let mut approval = d.clone();
    approval.key = "approval".into();
    approval.output = Out::Effect {
        node: NodeId(0),
        seq: 3,
        kind: EffectKind::PostApproval,
        head: None,
    };
    let mut adapter = server.adapter(&records, 1);
    assert!(matches!(
        adapter.reconcile(&approval).unwrap(),
        Reconciliation::Absent
    ));
    server.api.state.lock().unwrap().lose_response = Some("/check-runs");
    assert!(adapter.execute(&approval).is_err());
    drop(adapter);
    let mut adapter = server.adapter(&records, 1);
    assert!(matches!(
        adapter.reconcile(&approval).unwrap(),
        Reconciliation::Found(_)
    ));
    adapter.execute(&approval).unwrap();
    assert_eq!(server.api.state.lock().unwrap().checks.len(), 1);
    let mut wrong = approval.clone();
    wrong.head_sha = Some("b".repeat(40));
    assert!(matches!(
        adapter.reconcile(&wrong).unwrap(),
        Reconciliation::Absent
    ));
    server.api.state.lock().unwrap().checks[0]["app"]["id"] = json!(456);
    assert!(matches!(
        adapter.reconcile(&approval).unwrap(),
        Reconciliation::Absent
    ));
    server.api.state.lock().unwrap().checks[0]["app"]["id"] = json!(123);
    let mut open = d.clone();
    open.key = "open".into();
    open.output = Out::Effect {
        node: NodeId(0),
        seq: 4,
        kind: EffectKind::OpenPr,
        head: None,
    };
    server.api.state.lock().unwrap().lose_response = Some("/pulls");
    assert!(adapter.execute(&open).is_err());
    drop(adapter);
    let mut adapter = server.adapter(&records, 1);
    assert!(
        matches!(adapter.reconcile(&open).unwrap(),Reconciliation::Found(Completion {observations,..}) if matches!(observations.as_slice(),[Observation::PullRequest { number:100,..}]))
    );
    adapter.execute(&open).unwrap();
    assert_eq!(server.api.state.lock().unwrap().opens, 1);
    server.api.state.lock().unwrap().fail_reads = true;
    assert!(matches!(
        adapter.reconcile(&open).unwrap(),
        Reconciliation::Pending
    ));
    server.api.state.lock().unwrap().fail_reads = false;
    let mut close = d.clone();
    close.key = "close".into();
    close.output = Out::Effect {
        node: NodeId(0),
        seq: 5,
        kind: EffectKind::ClosePr,
        head: None,
    };
    server.api.state.lock().unwrap().lose_response = Some("/pulls/100");
    assert!(adapter.execute(&close).is_err());
    assert!(matches!(
        adapter.reconcile(&close).unwrap(),
        Reconciliation::Found(_)
    ));
    adapter.execute(&close).unwrap();
    assert_eq!(server.api.state.lock().unwrap().closes, 1);
    assert!(matches!(
        adapter.reconcile(&open).unwrap(),
        Reconciliation::Absent
    ));
    adapter.execute(&open).unwrap();
    adapter.execute(&open).unwrap();
    assert_eq!(server.api.state.lock().unwrap().reopens, 1);
    server.api.state.lock().unwrap().pull.as_mut().unwrap()["merged_at"] =
        json!("2026-10-30T00:00:00Z");
    adapter.execute(&close).unwrap();
    assert_eq!(server.api.state.lock().unwrap().closes, 1);
    let mut newer = server.adapter(&records, 2);
    assert!(matches!(
        newer.reconcile(&approval).unwrap(),
        Reconciliation::Found(_)
    ));
    assert!(newer.execute(&approval).is_err());
}

fn main() {
    if let Some(mode) = env::args().nth(1) {
        let python = prepare_workers();
        let mut database = support::Database::start();
        lifecycle(&database.create("publishing_regression"), &python, &mode);
        database.stop();
        println!("test publishing_{mode} ... ok");
        return;
    }
    publication_recovery();
    println!("test publication_lost_response_recovery ... ok");
    let python = prepare_workers();
    let mut database = support::Database::start();
    for mode in [
        "merged",
        "push",
        "rebase",
        "round-one-rebase",
        "unapproved",
        "changes",
        "new-attempt",
        "review-retry",
        "out-of-room",
        "out-of-room-fix",
        "out-of-room-wrap",
        "before-enqueue",
        "reports-latest",
        "reports-closed",
    ] {
        lifecycle(
            &database.create(&format!("publishing_{mode}")),
            &python,
            mode,
        );
        println!("test publishing_{mode} ... ok");
    }
    database.stop();
    println!("15 publishing scenarios passed; recorded upstreams; no nested submissions; no skips");
}
