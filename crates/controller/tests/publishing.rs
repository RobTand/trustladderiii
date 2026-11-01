mod prismabuild_fixture;
#[path = "publishing/adoption.rs"]
mod publishing_adoption;
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
    Bootstrap, Boundary, Controller, LabelledIssue, Repository,
    adapters::{
        self,
        github::APPROVAL_CHECK,
        prismabuild::{Check, CheckRun, Kind},
        workers::{
            Supervisor, WorkerOutputSource,
            writer::{FakeWriter, TextDraft, WriteRequest, Writer, WriterReply},
        },
    },
    cli::Input,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation, Upstream},
};
use tl3_github::{AppConfig, ClientOptions, GitHub, message::Policy};
use tl3_kernel::{EffectKind, NodeId, Outcome, Phase, Role};
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
        base_sha: tl3_worktrees::CommitId::new(base).unwrap(),

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
            &format!("refs/tl3/bases/{}", f.branch(d).as_str()),
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
fn workers(
    real: impl Executor + 'static,
    fake: impl Executor + 'static,
    fixture: Arc<Fixture>,
    mode: String,
) -> impl Executor {
    let add_add = mode == "add-add";
    let d5 = mode.starts_with("d5-") || (mode.starts_with("q46-") && mode != "q46-clean");
    support::workers::Workers::new(
        Some(Box::new(real)),
        fake,
        vec![Role::Review],
        move |d, c| {
            for o in &mut c.observations {
                match o {
                    Observation::Plan { paths, .. } => {
                        *paths = vec!["value.py".into()];
                        if add_add {
                            paths.push("pkg/new.py".into());
                        }
                    }
                    Observation::Made { sha, .. } => {
                        let checkout = fixture.adapter().checkout(d);
                        if d5 && fixture.source.join("main-moved.txt").exists() {
                            let target = git(&fixture.remote, &["rev-parse", "main"]);
                            let merge_head = Command::new("git")
                                .current_dir(&checkout)
                                .args(["rev-parse", "--verify", "MERGE_HEAD"])
                                .output()
                                .unwrap();
                            assert!(
                                merge_head.status.success(),
                                "the conflict maker must receive a merge onto current main, not the old clean tree"
                            );
                            assert_eq!(
                                String::from_utf8(merge_head.stdout).unwrap().trim(),
                                target
                            );
                            let conflict = fs::read_to_string(checkout.join("value.py")).unwrap();
                            assert!(
                                conflict.contains("<<<<<<<")
                                    && conflict.contains("return 2")
                                    && conflict.contains("return 3"),
                                "the maker must receive the conflicted file and both sides"
                            );
                            *sha = commit(&checkout, "value.py", "def value(): return 2\n");
                            assert_eq!(
                                git(&checkout, &["merge-base", sha, &target]),
                                target,
                                "the resolved maker head must contain main"
                            );
                        } else if add_add {
                            fs::create_dir_all(checkout.join("pkg")).unwrap();
                            if fixture.source.join("pkg/new.py").exists() {
                                *sha = commit(&checkout, "pkg/new.py", "def leaf(): return 2\n");
                            } else {
                                fs::write(checkout.join("pkg/new.py"), "def leaf(): return 1\n")
                                    .unwrap();
                                *sha = commit(&checkout, "value.py", "def value(): return 2\n");
                            }
                        } else if fs::read_to_string(checkout.join("value.py")).unwrap()
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
                    .insert(0, check_observation(&fixture, d, sha.clone()));
            }
        },
    )
}

struct State {
    pull: Option<Value>,
    proposed: Option<Value>,
    proposed_comments: Vec<Value>,
    checks: Vec<Value>,
    comments: Vec<Value>,
    operations_comments: Vec<Value>,
    labels: bool,
    conflicting_enqueue: bool,
    mergeable_override: Option<bool>,
    closed: bool,
    fail_reads: bool,
    not_found: Option<&'static str>,
    not_found_reads: usize,
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
            proposed: None,
            proposed_comments: vec![],
            checks: vec![],
            comments: vec![],
            operations_comments: vec![],
            labels: false,
            conflicting_enqueue: false,
            mergeable_override: None,
            closed: false,
            fail_reads: false,
            not_found: None,
            not_found_reads: 0,
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
            .is_some_and(|change| change.starts_with("For \"Fix the value\","))
        {
            assert!(
                !input.facts["change"]
                    .as_str()
                    .unwrap()
                    .contains("clear incident "),
                "the writer must receive only the incident summary"
            );
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
impl support::github::Routes for State {
    fn pulls(&self) -> Value {
        json!(self.pull.iter().collect::<Vec<_>>())
    }
    fn checks(&self, sha: &str) -> Value {
        let mut checks: Vec<_> = self
            .checks
            .iter()
            .filter(|c| c["head_sha"] == sha)
            .cloned()
            .collect();
        if self.labels && self.pull.as_ref().is_some_and(|p| p["head"]["sha"] == sha) {
            checks.push(recorded_check(self.queue_fixture, &json!(sha)));
        }
        json!(checks)
    }
    fn checks_mut(&mut self) -> Option<&mut Vec<Value>> {
        Some(&mut self.checks)
    }
    fn check_posted(&mut self) {
        self.operations.push("approve".into());
    }
    fn pull_mut(&mut self) -> Option<&mut Value> {
        self.pull.as_mut()
    }
    fn pull_patched(&mut self, body: &Value) {
        if body["state"] == "open" {
            self.reopens += 1;
        } else {
            self.closes += 1;
        }
    }
    fn created_status(&self) -> u16 {
        200
    }
}
impl Respond for Api {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        if r.method == "POST" && r.url.path() == "/v1/default/banks/tl3/memories" {
            let body: Value = serde_json::from_slice(&r.body).unwrap();
            assert_eq!(body["async"], true);
            assert_eq!(body["items"].as_array().unwrap().len(), 1);
            return ResponseTemplate::new(200).set_body_json(json!({
                "success": true, "async": true, "operation_id": body["operation_id"],
                "bank_id": "tl3", "items_count": 1,
            }));
        }
        let mut s = self.state.lock().unwrap();
        let method = r.method.as_str();
        let path = r.url.path();
        if let Some(response) = support::github::access_token(path, 1, "disposable") {
            return response;
        }
        if path.starts_with("/repos/RobTand/tl3-sandbox/")
            && s.not_found.is_some_and(|resource| path.ends_with(resource))
        {
            s.not_found_reads += usize::from(method == "GET");
            return ResponseTemplate::new(404)
                .set_body_json(json!({"message":"fixture resource not found"}));
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
        if path == "/pulls/17" {
            if method == "PATCH" {
                s.proposed.as_mut().unwrap()["state"] = body["state"].clone();
            }
            return ResponseTemplate::new(200).set_body_json(s.proposed.clone().unwrap());
        }
        if path == "/issues/17/comments" {
            if method == "POST" {
                let c = json!({"id":s.proposed_comments.len() + 101,"body":body["body"],"user":{"id":99,"login":"factory[bot]"},"created_at":"2026-10-31T00:00:00Z","updated_at":"2026-10-31T00:00:00Z","issue_url":"https://api.github.com/repos/RobTand/tl3-sandbox/issues/17"});
                s.proposed_comments.push(c.clone());
                return ResponseTemplate::new(201).set_body_json(c);
            }
            return ResponseTemplate::new(200).set_body_json(json!(s.proposed_comments));
        }
        let response = if let Some(response) =
            support::github::respond(r, r.url.path(), prefix, &mut *s)
        {
            response
        } else {
            let value = match (method, path) {
                ("POST", "/pulls") => {
                    s.opens += 1;
                    s.operations.push("open".into());
                    let head = git(
                        &self.fixture.remote,
                        &["rev-parse", body["head"].as_str().unwrap()],
                    );
                    let base = git(
                        &self.fixture.remote,
                        &["rev-parse", body["base"].as_str().unwrap()],
                    );
                    s.pull = Some(
                        json!({"body":body["body"],"number":100,"user":{"id":99,"login":"factory[bot]"},"state":"open","merged_at":null,"merge_commit_sha":null,
                    "head":{"ref":body["head"],"sha":head,"repo":{"id":1,"name":"tl3-sandbox","owner":{"id":1,"login":"RobTand"}}},
                    "base":{"ref":body["base"],"sha":base,"repo":null},"updated_at":"2026-10-30T00:00:00Z"}),
                    );
                    let merge = Command::new("git")
                        .current_dir(&self.fixture.remote)
                        .args([
                            "merge-tree",
                            "--write-tree",
                            s.pull.as_ref().unwrap()["base"]["ref"].as_str().unwrap(),
                            s.pull.as_ref().unwrap()["head"]["sha"].as_str().unwrap(),
                        ])
                        .output()
                        .unwrap();
                    s.pull.as_mut().unwrap()["mergeable"] = json!(merge.status.success());
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
                    let mergeable_override = s.mergeable_override;
                    if let Some(pull) = &mut s.pull {
                        let output = Command::new("git")
                            .current_dir(&self.fixture.remote)
                            .args([
                                "merge-tree",
                                "--write-tree",
                                pull["base"]["ref"].as_str().unwrap(),
                                pull["head"]["sha"].as_str().unwrap(),
                            ])
                            .output()
                            .unwrap();
                        let mergeable = mergeable_override.unwrap_or(output.status.success());
                        pull["mergeable"] = json!(mergeable);
                        pull["mergeable_state"] = json!(if mergeable { "clean" } else { "dirty" });
                    }
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
                            git(&self.fixture.remote, &["for-each-ref", "refs/heads/tl3"])
                                .is_empty()
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
                    let merge = Command::new("git")
                        .current_dir(&self.fixture.remote)
                        .args(["merge-tree", "--write-tree", "main", head.as_str().unwrap()])
                        .output()
                        .unwrap();
                    s.conflicting_enqueue = !merge.status.success();
                    s.operations.push("enqueue".into());
                    s.labels = true;
                    json!([{"name":"tl3-queue"}])
                }
                ("DELETE", "/issues/100/labels/tl3-queue") => {
                    s.labels = false;
                    json!([])
                }
                _ => panic!("unexpected mock request: {method} {path}"),
            };
            ResponseTemplate::new(200).set_body_json(value)
        };
        if method != "GET" && s.lose_response == Some(path) {
            s.lose_response = None;
            return ResponseTemplate::new(503);
        }
        response
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
        .with_repositories(&self.api.fixture.repositories)
        .unwrap()
    }
}
fn pb(root: &Path) -> adapters::prismabuild::Adapter {
    let (client, mut settings) = prismabuild_fixture::settings(root, "done", "http://127.0.0.1:9/");
    settings.records = root.join("pb-records");
    adapters::prismabuild::Adapter::new(client, settings).unwrap()
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
            if expected == (Phase::Planning { must_split: true })
                && c.world().unwrap().nodes[0].phase == expected
            {
                break;
            }
        }
        assert_ne!(
            c.world().unwrap().nodes[0].phase,
            Phase::AwaitingRob,
            "publication must recover without asking Rob"
        );
        source.poll_once(store, &sender).unwrap();
        let mut observed = false;
        for input in receiver.try_iter() {
            if let Input::Observation { key, observation } = input {
                c.observe(&key, observation).unwrap();
                changed = true;
                observed = true;
            }
        }
        let reached_planner = expected == (Phase::Planning { must_split: true }) && !observed;
        if (!changed || reached_planner) && c.world().unwrap().nodes[0].phase == expected {
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
    let mut fixture = Fixture::create(root.path());
    let mut config = tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
    if mode == "repo-conventions" {
        config.github.repos[0].branch_template = "prismabuild-{issue}-tl3-{node}".into();
        config.github.repos[0].base_branch = "master".into();
        fixture.repositories = config.github.repos.clone();
    }
    let f = Arc::new(fixture);
    let original = git(&f.source, &["rev-parse", "HEAD"]);
    fs::write(f.source.join("value.py"), "def value(): return 1\n").unwrap();
    fs::write(
        f.source.join("pyproject.toml"),
        "[tool.tl3.harness]\ncommand = ['python', '-m', 'pytest']\ntest-files = ['test_*.py']\n",
    )
    .unwrap();
    commit(
        &f.source,
        "test_value.py",
        "from value import value\ndef test_value(): assert value() == 2\n",
    );
    assert_eq!(
        git(
            &f.source,
            &["rev-list", "--count", &format!("{original}..HEAD")]
        ),
        "1",
        "publishing setup needs one base commit, not one per file"
    );
    git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
    if mode == "repo-conventions" {
        for path in [&f.source, &f.remote, &f.repo] {
            git(path, &["branch", "-m", "main", "master"]);
        }
    }
    let server = Server::new(f.clone());

    config.store = settings.clone();
    if mode.starts_with("out-of-room") || mode.starts_with("review-reset") {
        let mut primary = config.models.maker[2].clone();
        primary.name = "review-primary".into();
        let mut backup = config.models.maker[0].clone();
        backup.name = "review-backup".into();
        config.models.reviewer = if mode == "out-of-room-last" {
            vec![backup, primary]
        } else {
            vec![primary, backup]
        };
    }
    config.hindsight.endpoint = server.server.uri().parse().unwrap();
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
    if mode == "repo-conventions" {
        let mut adapter = f.adapter().with_repositories(&config.github.repos).unwrap();
        let mut preparation = f.delivery(EffectKind::PrepareWorktree, 999);
        preparation.epoch = 0;
        adapter
            .execute(&preparation)
            .expect("configured master base must prepare without a main branch");
    }
    let supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let mut source = supervisor.source();
    let mut registry = adapters::simulation(&root.path().join("fakes")).unwrap();
    registry.register(
        Upstream::Workers,
        workers(
            supervisor,
            adapters::workers::Fake::new(&root.path().join("fakes")),
            f.clone(),
            mode.into(),
        ),
    );
    registry.register(
        Upstream::Worktrees,
        f.adapter().with_repositories(&config.github.repos).unwrap(),
    );
    registry.register(
        Upstream::GitHub,
        server
            .adapter(&root.path().join("github"), 0)
            .with_repositories(&config.github.repos)
            .unwrap(),
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
    if mode.starts_with("q46-") {
        let (sender, receiver) = mpsc::channel();
        let start = Instant::now();
        loop {
            let node = c.world().unwrap().nodes[0];
            let reached = match mode {
                "q46-checking" | "q46-clean" => node.phase == Phase::Checking,
                "q46-reviewing" => node.phase == Phase::Reviewing,
                "q46-enqueue" => node.effect.is_some_and(|e| e.kind == EffectKind::Enqueue),
                _ => panic!("unknown Q46 mode"),
            };
            if reached {
                break;
            }
            c.drive_once().unwrap();
            source.poll_once(&mut store, &sender).unwrap();
            for input in receiver.try_iter() {
                tl3_controller::ingress::apply(&mut c, input).unwrap();
            }
            assert!(start.elapsed() < Duration::from_secs(90));
        }
        if mode != "q46-clean" {
            commit(&f.source, "value.py", "def value(): return 3\n");
        }
        commit(
            &f.source,
            "main-moved.txt",
            "main moved before publication\n",
        );
        git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
        if mode == "q46-enqueue" {
            let mut state = server.api.state.lock().unwrap();
            state.pull.as_mut().unwrap()["mergeable"] = json!(false);
            state.pull.as_mut().unwrap()["mergeable_state"] = json!("dirty");
        }
        if mode == "q46-enqueue" {
            for _ in 0..100 {
                assert!(c.drive_once().unwrap());
                assert!(
                    !server.api.state.lock().unwrap().conflicting_enqueue,
                    "a conflicting checked head must rebase before enqueue"
                );
                if c.world().unwrap().nodes[0].phase == Phase::Rebasing {
                    break;
                }
            }
            assert_eq!(c.world().unwrap().nodes[0].phase, Phase::Rebasing);
        }
    }
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
                &format!("HEAD:refs/heads/{}", f.branch(&d).as_str()),
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
        drain(
            &mut c,
            &mut source,
            &mut store,
            Phase::Planning { must_split: true },
        );
        let n = c.world().unwrap().nodes[0];
        assert_eq!(
            n.run_failures, 0,
            "Q44 exhaustion is not an infrastructure failure"
        );
        assert_eq!(n.rechecks, 1);
        assert!(!n.holds);
        assert_eq!(n.credit.balance(), 3);
        let reviews: Vec<Value> = fs::read_to_string(root.path().join("reviews.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(reviews.len(), if mode == "out-of-room-fix" { 3 } else { 2 });
        let first = &reviews[reviews.len() - 2];
        let fresh = &reviews[reviews.len() - 1];
        assert_eq!(fresh["resume_session"], false);
        assert_ne!(first["session_file"], fresh["session_file"]);
        assert_ne!(
            first["operation"]["generation"],
            fresh["operation"]["generation"]
        );
        assert_eq!(first["model"]["name"], "review-primary");
        assert_eq!(
            fresh["model"]["name"],
            if mode == "out-of-room-last" {
                "review-primary"
            } else {
                "review-backup"
            }
        );
        assert_eq!(
            fresh["inputs"]["base_sha"],
            reviews[0]["inputs"]["base_sha"]
        );
        return;
    }
    drain(&mut c, &mut source, &mut store, Phase::Queued);
    tl3_controller::memory::flush(&mut store, &config.hindsight)
        .expect("publishing fixture must acknowledge memories instead of retrying a closed port");
    if mode == "repo-conventions" {
        let state = server.api.state.lock().unwrap();
        let pull = state.pull.as_ref().unwrap();
        assert_eq!(pull["head"]["ref"], "prismabuild-42-tl3-0");
        assert_eq!(pull["base"]["ref"], "master");
        assert!(
            pull["body"].as_str().unwrap().contains("Refs #42"),
            "publication must reference the request without closing it before TL3 finishes all leaves"
        );
        if let Some(path) = env::var_os("TL3_REPO_CONVENTIONS_PULL") {
            let mut pull = pull.clone();
            pull["created_at"] = json!("2026-11-01T00:00:00Z");
            fs::write(path, serde_json::to_vec_pretty(&pull).unwrap()).unwrap();
        }
        let base = git(&f.remote, &["rev-parse", "master"]);
        assert_eq!(
            git(&f.remote, &["merge-base", &base, "prismabuild-42-tl3-0"]),
            base
        );
    }
    let first_head = server.api.state.lock().unwrap().pull.as_ref().unwrap()["head"]["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    let credit = c.world().unwrap().nodes[0].credit.balance();
    if mode.starts_with("q46-") {
        let node = c.world().unwrap().nodes[0];
        assert_eq!(node.credit.spent, u32::from(mode != "q46-clean"));
        assert_eq!(node.rechecks, 0);
        assert!(node.question.is_none());
        let main = git(&f.remote, &["rev-parse", "main"]);
        let d = f.delivery(EffectKind::Push, 100);
        let checkout = f.adapter().checkout(&d);
        let contains_main = git(&checkout, &["merge-base", &first_head, &main]) == main;
        assert_eq!(contains_main, mode != "q46-clean");
        assert_eq!(
            fs::read_to_string(root.path().join("reviews.jsonl"))
                .unwrap()
                .lines()
                .count(),
            if mode == "q46-clean" { 1 } else { 2 }
        );
    }

    let d = f.delivery(EffectKind::Push, 100);
    if matches!(mode, "push" | "reports-latest" | "review-reset-push") {
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
                &format!("HEAD:refs/heads/{}", f.branch(&d).as_str()),
            ],
        );

        webhook(&mut c, &server, "synchronize");
        if mode == "review-reset-push" {
            while c.drive_once().unwrap() {}
            assert!(
                !c.world().unwrap().nodes[0].review_restarted,
                "outside push starts a new review"
            );
        }
        drain(&mut c, &mut source, &mut store, Phase::Queued);
    } else if mode.starts_with("d5-") {
        let checkout = f.adapter().checkout(&d);
        if mode == "d5-conflict" {
            commit(&f.source, "value.py", "def value(): return 3\n");
        }
        let target = commit(&f.source, "main-moved.txt", "main moved while queued\n");
        git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
        {
            let mut state = server.api.state.lock().unwrap();
            state.queue_fixture = "dequeued";
            state.mergeable_override = Some(false);
            let pull = state.pull.as_mut().unwrap();
            pull["mergeable"] = json!(false);
            pull["mergeable_state"] = json!("dirty");
        }
        for _ in 0..10 {
            if c.world().unwrap().nodes[0].phase != Phase::Queued {
                break;
            }
            assert!(c.drive_once().unwrap());
        }
        assert_eq!(c.world().unwrap().nodes[0].phase, Phase::Rebasing);
        for _ in 0..100 {
            assert!(c.drive_once().unwrap());
            if c.world().unwrap().nodes[0].phase == Phase::Checking {
                break;
            }
        }
        assert_eq!(c.world().unwrap().nodes[0].phase, Phase::Checking);
        let head = f.head(&d);
        assert_eq!(
            git(&checkout, &["merge-base", &head, &target]),
            target,
            "the checked head must contain current main"
        );
        assert_eq!(
            base(&f, &d),
            target,
            "checks must compare with the rebased base"
        );
        assert_eq!(
            c.world().unwrap().nodes[0].credit.balance(),
            credit - if mode == "d5-conflict" { 1 } else { 0 }
        );
        {
            let mut state = server.api.state.lock().unwrap();
            state.queue_fixture = "running";
            state.mergeable_override = None;
            let pull = state.pull.as_mut().unwrap();
            pull["mergeable"] = json!(true);
            pull["mergeable_state"] = json!("clean");
        }
        drain(&mut c, &mut source, &mut store, Phase::Queued);
        let published = git(&f.remote, &["rev-parse", f.branch(&d).as_str()]);
        assert_eq!(published, head);
        assert_eq!(
            git(&checkout, &["merge-base", &published, &target]),
            target,
            "the published head must contain current main"
        );
        let reviews: Vec<Value> = fs::read_to_string(root.path().join("reviews.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(reviews.len(), 2, "the resolved head needs a fresh review");
        assert_eq!(reviews[1]["inputs"]["head_sha"], head);
        assert_eq!(reviews[1]["inputs"]["check_base_sha"], target);
        assert!(
            !reviews[1]["inputs"]["checks"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    } else if mode == "add-add" {
        let checkout = f.adapter().checkout(&d);
        assert_eq!(
            git(&checkout, &["show", "HEAD:pkg/new.py"]),
            "def leaf(): return 1"
        );
        assert!(!f.source.join("pkg/new.py").exists());
        fs::create_dir_all(f.source.join("pkg")).unwrap();
        let target = commit(&f.source, "pkg/new.py", "def upstream(): return 3\n");
        git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
        {
            let mut state = server.api.state.lock().unwrap();
            state.queue_fixture = "dequeued";
            let pull = state.pull.as_mut().unwrap();
            pull["mergeable"] = json!(false);
            pull["mergeable_state"] = json!("dirty");
        }
        for _ in 0..10 {
            if c.world().unwrap().nodes[0].phase != Phase::Queued {
                break;
            }
            assert!(
                c.drive_once().unwrap(),
                "PR_DEQUEUED must yield a Conflict fact"
            );
        }
        assert_eq!(c.world().unwrap().nodes[0].phase, Phase::Rebasing);
        let mut saw_maker_fallback = false;
        for _ in 0..100 {
            assert!(c.drive_once().unwrap());
            let phase = c.world().unwrap().nodes[0].phase;
            saw_maker_fallback |= phase == Phase::Making;
            if phase == Phase::Checking {
                break;
            }
        }
        assert!(
            saw_maker_fallback,
            "the real add/add rebase must fall back to the maker"
        );
        assert_eq!(c.world().unwrap().nodes[0].phase, Phase::Checking);
        let new_head = f.head(&d);
        assert_ne!(new_head, first_head);
        assert_eq!(git(&checkout, &["merge-base", &new_head, &target]), target);
        assert_eq!(
            git(&checkout, &["show", "HEAD:pkg/new.py"]),
            "def leaf(): return 2"
        );
        assert!(git(&checkout, &["status", "--porcelain"]).is_empty());
        {
            let mut state = server.api.state.lock().unwrap();
            state.queue_fixture = "running";
            let pull = state.pull.as_mut().unwrap();
            pull["mergeable"] = json!(true);
            pull["mergeable_state"] = json!("clean");
        }
        drain(&mut c, &mut source, &mut store, Phase::Queued);
        let reviews: Vec<Value> = fs::read_to_string(root.path().join("reviews.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            reviews.len(),
            2,
            "the new head needs a fresh independent review"
        );
        assert_eq!(reviews[1]["inputs"]["head_sha"], new_head);
        assert!(
            !reviews[1]["inputs"]["checks"]
                .as_array()
                .unwrap()
                .is_empty(),
            "review must see checks on the new head"
        );
        assert!(
            c.snapshot()
                .unwrap()
                .rows
                .iter()
                .all(|row| !matches!(row.table.as_str(), "incidents" | "questions"))
        );
    } else if matches!(
        mode,
        "rebase" | "round-one-rebase" | "review-reset-rebase" | "repo-conventions"
    ) {
        f.advance_main(false);
        server.api.state.lock().unwrap().queue_fixture = "conflict";
        let new_target = git(
            &f.remote,
            &["rev-parse", &config.github.repos[0].base_branch],
        );
        loop {
            c.drive_once().unwrap();
            let n = c.world().unwrap().nodes[0];
            if n.phase == Phase::Checking {
                if mode == "review-reset-rebase" {
                    assert!(!n.review_restarted, "clean rebase starts a new review");
                }
                let head = f.head(&d);
                assert_ne!(head, first_head);
                assert_eq!(
                    git(&f.repo, &["merge-base", &head, &new_target]),
                    new_target
                );
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
    if mode.starts_with("review-reset") {
        assert_eq!(reviews.len(), 4);
        assert_ne!(
            reviews[0]["inputs"]["head_sha"],
            reviews[2]["inputs"]["head_sha"]
        );
        for pair in reviews.as_chunks::<2>().0 {
            assert_eq!(pair[0]["model"]["name"], "review-primary");
            assert_eq!(pair[1]["model"]["name"], "review-backup");
            assert_eq!(pair[0]["resume_session"], false);
            assert_eq!(pair[1]["resume_session"], false);
            assert_ne!(pair[0]["session_file"], pair[1]["session_file"]);
        }
        assert_eq!(
            reviews[2]["last_accepted_generation"],
            reviews[1]["operation"]["generation"]
        );
        assert_eq!(
            reviews[3]["last_accepted_generation"],
            reviews[2]["operation"]["generation"]
        );
        assert_eq!(c.world().unwrap().nodes[0].rechecks, 3);
        return;
    }
    assert_eq!(
        reviews.len(),
        if matches!(mode, "new-attempt" | "review-retry" | "round-one-rebase") {
            3
        } else if matches!(
            mode,
            "push"
                | "repo-conventions"
                | "rebase"
                | "changes"
                | "before-enqueue"
                | "reports-latest"
                | "add-add"
                | "d5-clean"
                | "d5-conflict"
                | "q46-checking"
                | "q46-reviewing"
                | "q46-enqueue"
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
            !snapshot.rows.iter().any(|r| matches!(
                r.table.as_str(),
                "review_reports" | "reviews" | "plan_reports"
            )),
            "unused worker reports must not accumulate"
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
    if mode == "repo-conventions" {
        assert!(
            git(
                &f.remote,
                &["for-each-ref", "refs/heads/prismabuild-42-tl3-0"]
            )
            .is_empty()
        );
    }
    assert!(!root.path().join("pb-records/0").exists());
    assert!(store.worker_progress(None).unwrap().is_empty());
    let state = server.api.state.lock().unwrap();
    assert!(state.closed);
    assert_eq!(state.opens, 1);
    assert_eq!(
        state.checks.len(),
        if mode == "round-one-rebase" {
            2
        } else if matches!(
            mode,
            "changes" | "new-attempt" | "review-retry" | "q46-checking" | "q46-reviewing"
        ) {
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
fn publication_open_conflict() {
    let root = tempfile::tempdir().unwrap();
    let f = Arc::new(Fixture::create(root.path()));
    let prepare = f.delivery(EffectKind::PrepareWorktree, 1);
    f.adapter().execute(&prepare).unwrap();
    let head = commit(&f.adapter().checkout(&prepare), "shared.txt", "leaf side\n");
    let mut push = f.delivery(EffectKind::Push, 2);
    push.head_sha = Some(head);
    f.adapter().execute(&push).unwrap();
    f.advance_main(true);
    let server = Server::new(f);
    let mut open = push.clone();
    open.key = "open-conflict".into();
    open.output = Out::Effect {
        node: NodeId(0),
        seq: 3,
        kind: EffectKind::OpenPr,
        head: None,
    };
    let mut adapter = server.adapter(&root.path().join("github"), 1);
    let result = adapter.execute(&open).unwrap();
    assert!(
        result.observations.iter().any(|o| matches!(
            o,
            Observation::Kernel(tl3_kernel::Event::Fact {
                fact: tl3_kernel::Fact::Conflict,
                ..
            })
        )),
        "opening a conflicting PR must report Conflict: {result:?}"
    );
    assert!(
        result
            .observations
            .iter()
            .any(|o| matches!(o, Observation::PullRequest { number: 100, .. })),
        "keep the PR binding for re-publication and teardown"
    );
    assert!(
        matches!(adapter.reconcile(&open).unwrap(), Reconciliation::Found(c)
        if c.observations.iter().any(|o| matches!(o, Observation::Kernel(tl3_kernel::Event::Fact { fact: tl3_kernel::Fact::Conflict, .. }))))
    );
    assert!(!server.api.state.lock().unwrap().labels);
}
fn publication_requires_conventions() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Arc::new(Fixture::create(root.path()));
    let prepare = fixture.delivery(EffectKind::PrepareWorktree, 1);
    fixture.adapter().execute(&prepare).unwrap();
    let mut push = fixture.delivery(EffectKind::Push, 2);
    push.head_sha = Some(fixture.head(&prepare));
    fixture.adapter().execute(&push).unwrap();
    let server = Server::new(fixture.clone());
    let mut adapter = server
        .adapter(&root.path().join("github"), 1)
        .with_repositories(&[])
        .unwrap();
    let result = adapter.execute(&fixture.delivery(EffectKind::OpenPr, 3));
    assert!(
        matches!(
            result,
            Err(tl3_controller::Error::Invalid(
                "repository branch conventions are not configured"
            ))
        ),
        "an unconfigured publisher must reject publication, got {result:?}"
    );
    assert!(
        server
            .runtime
            .block_on(server.server.received_requests())
            .unwrap()
            .is_empty()
    );
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

fn publication_404_look(settings: &tl3_config::Store, case: &str) {
    let kind = if case == "approval" {
        EffectKind::PostApproval
    } else {
        EffectKind::OpenPr
    };
    let root = tempfile::tempdir().unwrap();
    let server = Server::new(Arc::new(Fixture::create(root.path())));
    server.api.state.lock().unwrap().not_found = Some(if case == "approval" {
        "check-runs"
    } else {
        "pulls"
    });
    let registry = || {
        let mut registry = adapters::simulation(root.path()).unwrap();
        registry.register(
            Upstream::GitHub,
            server.adapter(&root.path().join("github"), 0),
        );
        registry
    };
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry(), bootstrap()).unwrap();
    controller
        .intake(LabelledIssue {
            delivery: "publication-404".into(),
            owner: repo().owner,
            repo: repo().name,
            installation: repo().installation,
            number: 42,
            title: "Fix the value".into(),
            actor: 1,
            label: "tl3".into(),
        })
        .unwrap();
    let attempts = std::cell::Cell::new(0);
    let settle = |controller: &mut Controller| {
        for _ in 0..300 {
            if !controller.drive_once_with(|boundary, key| {
                if boundary == Boundary::BeforeDispatch {
                    let effect = Store::open(settings).unwrap().effect(key).unwrap().unwrap();
                    let delivery: Delivery = serde_json::from_value(effect.payload).unwrap();
                    if matches!(delivery.output, Out::Effect { kind: actual, .. } if actual == kind) {
                        attempts.set(attempts.get() + 1);
                    }
                }
            }).unwrap() {
                return;
            }
        }
        panic!("publication 404 did not settle");
    };
    settle(&mut controller);
    assert_eq!(attempts.get(), 1);
    let pending = controller
        .snapshot()
        .unwrap()
        .pending_effects
        .into_iter()
        .find(|effect| {
            let delivery: Delivery = serde_json::from_value(effect.payload.clone()).unwrap();
            matches!(delivery.output, Out::Effect { kind: actual, .. } if actual == kind)
        })
        .expect("the first publication 404 must be held");
    assert_eq!(pending.state, tl3_store::EffectState::Uncertain);
    if kind == EffectKind::OpenPr {
        let delivery: Delivery = serde_json::from_value(pending.payload.clone()).unwrap();
        assert_eq!(delivery.pull_request, None);
    }
    if case == "open-restart" {
        drop(controller);
        controller =
            Controller::open(Store::open(settings).unwrap(), registry(), bootstrap()).unwrap();
    }
    let reads = server.api.state.lock().unwrap().not_found_reads;
    controller.look_key(&pending.key).unwrap();
    assert!(
        server.api.state.lock().unwrap().not_found_reads > reads,
        "the real look must read the still-missing resource"
    );
    assert_eq!(
        controller.world().unwrap().nodes[0].fx_failures,
        1,
        "a held publication's 404 look must permit its one retry"
    );
    settle(&mut controller);
    assert_eq!(
        attempts.get(),
        2,
        "persistent 404 must dispatch exactly one retry"
    );
    let snapshot = controller.snapshot().unwrap();
    assert!(
        snapshot.rows.iter().any(|row| row.table == "incidents"
            && row.value["signature"] == json!({"upstream":"GitHub", "class":"not_found"})),
        "persistent publication 404 must end permanent not_found"
    );
    assert!(
        !snapshot
            .rows
            .iter()
            .any(|row| row.table == "incidents" && row.value["signature"]["class"] == "setup")
    );
    for _ in 0..5 {
        assert!(
            !controller.drive_once().unwrap(),
            "persistent publication 404 must not loop"
        );
    }
    assert_eq!(attempts.get(), 2);
    println!("test publication_404_look_{case} ... ok");
}

fn main() {
    if env::args().nth(1).as_deref() == Some("q46-open") {
        publication_open_conflict();
        println!("test publication_open_conflict ... ok");
        return;
    }
    if env::args().nth(1).as_deref() == Some("--require-conventions") {
        publication_requires_conventions();
        println!("test publication_requires_conventions ... ok");
        return;
    }
    if let Some(case) = env::args()
        .nth(1)
        .and_then(|arg| arg.strip_prefix("--effect-404=").map(str::to_owned))
    {
        let mut database = support::Database::start();
        publication_404_look(&database.create("publication_404"), &case);
        database.stop();
        return;
    }
    if env::args().len() == 1 || env::args().nth(1).as_deref() == Some("--router-regression") {
        let root = tempfile::tempdir().unwrap();
        let mut router = workers(
            support::executor_probe::Probe,
            support::executor_probe::Probe,
            Arc::new(Fixture::create(root.path())),
            "merged".into(),
        );
        support::executor_probe::assert_retire_delivery(&mut router);
        println!("test router_retire_delivery ... ok");
        if std::env::args().len() > 1 {
            return;
        }
    }

    if let Some(mode) = env::args().nth(1) {
        let python = prepare_workers();
        let mut database = support::Database::start();
        if mode.starts_with("adoption-") {
            publishing_adoption::run(&database.create("adoption"), &python, &mode);
        } else {
            lifecycle(&database.create("publishing_regression"), &python, &mode);
        }
        database.stop();
        println!("test publishing_{mode} ... ok");
        return;
    }
    publication_open_conflict();
    println!("test publication_open_conflict ... ok");
    publication_requires_conventions();
    println!("test publication_requires_conventions ... ok");
    publication_recovery();
    println!("test publication_lost_response_recovery ... ok");
    let python = prepare_workers();
    let mut database = support::Database::start();
    for case in ["open", "approval", "open-restart"] {
        publication_404_look(&database.create(&format!("publication_404_{case}")), case);
    }
    for mode in [
        "repo-conventions",
        "merged",
        "push",
        "rebase",
        "round-one-rebase",
        "add-add",
        "q46-checking",
        "q46-reviewing",
        "q46-enqueue",
        "q46-clean",
        "d5-clean",
        "d5-conflict",
        "unapproved",
        "changes",
        "new-attempt",
        "review-retry",
        "out-of-room",
        "out-of-room-fix",
        "out-of-room-last",
        "review-reset-push",
        "review-reset-rebase",
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
    for mode in [
        "adoption-pass",
        "adoption-fix",
        "adoption-extras",
        "adoption-negative",
    ] {
        publishing_adoption::run(&database.create(&mode.replace('-', "_")), &python, mode);
        println!("test {mode} ... ok");
    }
    database.stop();
    println!("35 publishing scenarios passed; recorded upstreams; no nested submissions; no skips");
}
