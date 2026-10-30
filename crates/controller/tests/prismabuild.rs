#[expect(
    dead_code,
    reason = "shared fixture also has status-reader restart helpers"
)]
#[path = "../../store/tests/support/mod.rs"]
mod support;
use std::{
    collections::BTreeSet,
    fs,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository,
    adapters::{
        self,
        prismabuild::{Adapter, Check, CheckRun, Kind, Settings},
    },
    executor::{Completion, Delivery, Executor, Observation, Reconciliation, Upstream},
};
use tl3_kernel::{Command as Control, EffectKind, Event, NodeId, Out, Phase};
use tl3_prismabuild::{Demand, Options, PrismaBuild, Tool};

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
fn fixture_repo(root: &Path, outside: bool) -> (PathBuf, String, String) {
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "-b", "main", "--template="]);
    fs::write(
        repo.join("pyproject.toml"),
        "[tool.tl3.harness]\ncommand = ['python', '-m', 'pytest']\ntest-files = ['test_*.py']\n",
    )
    .unwrap();
    fs::write(repo.join("value.py"), "def value(): return 1\n").unwrap();
    fs::write(
        repo.join("test_value.py"),
        "from value import value\ndef test_value(): assert value() == 2\n",
    )
    .unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "base"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("value.py"), "def value(): return 2\n").unwrap();
    if outside {
        fs::write(repo.join("outside.py"), "outside = True\n").unwrap();
    }
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "head"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    // Dirty and untracked maker files must not enter the snapshot.
    fs::write(
        repo.join("value.py"),
        "raise RuntimeError('dirty maker file')\n",
    )
    .unwrap();
    fs::write(repo.join("untracked.py"), "not committed\n").unwrap();
    (repo, base, head)
}
fn repository() -> Repository {
    Repository {
        owner: "RobTand".into(),
        name: "tl3-sandbox".into(),
        installation: 1,
    }
}
fn run(root: &Path, outside: bool) -> Delivery {
    let (repo, base, head) = fixture_repo(root, outside);
    Delivery {
        key: "effect:0:5".into(),
        epoch: 1,
        output: Out::Effect {
            node: NodeId(0),
            seq: 5,
            kind: EffectKind::SubmitChecks,
            head: None,
        },
        generation: 4,
        head_sha: Some(head),
        repository: repository(),
        issue: 42,
        pull_request: None,
        paths: vec!["value.py".into()],
        attempt: 1,
        round: 0,
        job_generation: Some(4),
        checks: Some(CheckRun {
            repository: repo,
            base_sha: base,
            plan: vec![Check {
                name: "value".into(),
                kind: Kind::Demonstrating,
                tests: vec!["test_value.py::test_value".into()],
            }],
        }),
    }
}
fn fixture(root: &Path, mode: &str) -> Adapter {
    fixture_with_bell(root, mode, "http://127.0.0.1:9/")
}
fn fixture_with_bell(root: &Path, mode: &str, bell_url: &str) -> Adapter {
    let queue = root.join("queue");
    fs::create_dir_all(&queue).unwrap();
    for state in ["ready", "claimed", "done", "failed", "withdrawn"] {
        fs::create_dir_all(queue.join(state)).unwrap();
    }
    fs::write(queue.join("mode"), mode).unwrap();
    let tool = Tool {
        program: "python3".into(),
        args: vec![
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/pb.py")
                .into(),
            queue.clone().into(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../prismabuild/tests/fixtures/queue")
                .into(),
        ],
    };
    let client = PrismaBuild::new(Options {
        queue_root: queue,
        tool: tool.clone(),
        bell_url: bell_url.into(),
    })
    .unwrap();
    Adapter::new(
        client,
        Settings {
            records: root.join("records"),
            harness: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../harness/src/tl3_harness"),
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
fn observed(completion: Completion) -> (bool, bool, Vec<String>) {
    assert_eq!(completion.observations.len(), 1);
    match completion.observations.into_iter().next().unwrap() {
        Observation::Checks {
            report,
            pass,
            changed_paths,
            ..
        } => (report, pass, changed_paths),
        _ => panic!("expected check evidence"),
    }
}
fn completion(adapter: &mut Adapter, delivery: &Delivery) -> Completion {
    match adapter.reconcile(delivery).unwrap() {
        Reconciliation::Found(completion) => completion,
        _ => panic!("expected ending"),
    }
}
fn recorded_endings() {
    for (mode, expected) in [
        ("done", (true, true)),
        ("failed", (true, false)),
        ("withdrawn", (false, false)),
        ("no_report", (false, false)),
        ("check_fail", (true, false)),
        ("infrastructure", (false, false)),
        ("bad_report", (false, false)),
        ("wrong_head", (false, false)),
    ] {
        let root = tempfile::tempdir().unwrap();
        let delivery = run(root.path(), false);
        let mut adapter = fixture(root.path(), mode);
        assert!(adapter.execute(&delivery).unwrap().observations.is_empty());
        let (report, pass, paths) = observed(completion(&mut adapter, &delivery));
        assert_eq!((report, pass), expected, "{mode}");
        assert_eq!(paths, ["value.py"]);
        let calls = fs::read_to_string(root.path().join("queue/calls.jsonl")).unwrap();
        assert!(
            calls.contains("--seal-only")
                && calls.contains("--max-attempts\", \"1")
                && !calls.contains("--timeout-s")
        );
    }
}
fn pending_and_withdrawal() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut adapter = fixture(root.path(), "pending");
    adapter.execute(&delivery).unwrap();
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Pending
    ));
    let mut withdraw = delivery.clone();
    withdraw.key = "effect:0:6".into();
    withdraw.output = Out::Effect {
        node: NodeId(0),
        seq: 6,
        kind: EffectKind::WithdrawJob,
        head: None,
    };
    fs::write(root.path().join("queue/cleanup_pending"), "true").unwrap();
    assert!(adapter.execute(&withdraw).is_err());
    assert!(matches!(
        adapter.reconcile(&withdraw).unwrap(),
        Reconciliation::Pending
    ));
    fs::remove_file(root.path().join("queue/cleanup_pending")).unwrap();
    assert!(matches!(
        adapter.reconcile(&withdraw).unwrap(),
        Reconciliation::Found(_)
    ));
    let mut older = delivery.clone();
    older.epoch = 0;
    assert!(fixture(root.path(), "withdrawn").execute(&older).is_err());
    assert!(matches!(
        adapter.reconcile(&older).unwrap(),
        Reconciliation::Found(_)
    ));
}
fn unknown_outcomes() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut adapter = fixture(root.path(), "pending");
    fs::write(root.path().join("queue/lost_response"), "true").unwrap();
    assert!(adapter.execute(&delivery).is_err());
    fs::remove_file(root.path().join("queue/lost_response")).unwrap();
    adapter.execute(&delivery).unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("queue/publications"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let key = adapter.action_key(&delivery.key).unwrap().unwrap();
    fs::write(
        root.path().join(format!("queue/ready/{key}.json")),
        "malformed",
    )
    .unwrap();
    assert!(adapter.reconcile(&delivery).is_err());
    assert_eq!(
        fs::read_to_string(root.path().join("queue/publications"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}
fn crash_reuses_seal() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    fs::write(
        root.path().join("delivery.json"),
        serde_json::to_vec(&delivery).unwrap(),
    )
    .unwrap();
    fs::create_dir_all(root.path().join("queue")).unwrap();
    fs::write(root.path().join("queue/hold_publish_response"), "hold").unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--kill-child")
        .arg(root.path())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !root.path().join("queue/at_publication").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "fixture never reached publication"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "adapter child exited before publication"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    let mut adapter = fixture(root.path(), "pending");
    let key = adapter.action_key(&delivery.key).unwrap().unwrap();
    // Re-execution can no longer depend on the old source tree or configured harness.
    fs::remove_dir_all(&delivery.checks.as_ref().unwrap().repository).unwrap();
    adapter.execute(&delivery).unwrap();
    assert_eq!(adapter.action_key(&delivery.key).unwrap(), Some(key));
    assert_eq!(
        fs::read_to_string(root.path().join("queue/publications"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let calls: Vec<Vec<String>> = fs::read_to_string(root.path().join("queue/calls.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.contains(&"--seal-only".to_owned()))
            .count(),
        1
    );
}
struct BellRequest {
    line: String,
    response: mpsc::SyncSender<serde_json::Value>,
}
impl BellRequest {
    fn reply(self, seq: u64, events: serde_json::Value) {
        self.response
            .send(serde_json::json!({
                "epoch": "0123456789abcdef0123456789abcdef", "seq": seq, "events": events
            }))
            .unwrap();
    }
}
struct Bell {
    url: String,
    address: std::net::SocketAddr,
    requests: Option<mpsc::Receiver<BellRequest>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Bell {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let (sender, requests) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut handlers = vec![];
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                if stopping.load(Ordering::Relaxed) {
                    break;
                }
                let sender = sender.clone();
                handlers.push(thread::spawn(move || {
                    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                    let mut reader = BufReader::new(&stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 { return; }
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) == 0 { return; }
                        if header == "\r\n" { break; }
                    }
                    let (response, receiver) = mpsc::sync_channel(0);
                    if sender.send(BellRequest { line, response }).is_err() { return; }
                    let Ok(response) = receiver.recv() else { return; };
                    let body = response.to_string();
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                }));
            }
            for handler in handlers {
                handler.join().unwrap();
            }
        });
        Self {
            url: format!("http://{address}/"),
            address,
            requests: Some(requests),
            stop,
            thread: Some(thread),
        }
    }
    fn request(&self) -> BellRequest {
        self.requests
            .as_ref()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
    }
}
impl Drop for Bell {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.requests.take();
        let _ = TcpStream::connect(self.address);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn source_thread_delivers_once_and_restarts_idle() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let bell = Bell::new();
    let mut adapter = fixture_with_bell(root.path(), "pending", &bell.url);
    let (sender, receiver) = mpsc::channel();
    let running = adapter.source().spawn(sender.clone());
    // There are no startup jobs. Only execute's channel notification can introduce this key.
    adapter.execute(&delivery).unwrap();
    let key = adapter.action_key(&delivery.key).unwrap().unwrap();
    let now = bell.request();
    assert!(now.line.contains("/v1/now"));
    now.reply(0, serde_json::json!([]));
    let waiting = bell.request();
    assert!(waiting.line.contains("/v1/wait"));
    assert!(waiting.line.contains(key.as_str()));
    finish(root.path(), &key.to_string(), "done");
    waiting.reply(
        1,
        serde_json::json!([{"seq": 1, "dir": "done", "key": key}]),
    );
    let observation = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(matches!(
        observation,
        tl3_controller::cli::Input::Observation {
            observation: Observation::Checks {
                report: true,
                pass: true,
                ..
            },
            ..
        }
    ));
    drop(running);
    assert_eq!(
        fs::read_dir(root.path().join("records"))
            .unwrap()
            .filter(|e| e
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|v| v == "done"))
            .count(),
        1
    );
    assert!(
        !fs::read_dir(root.path().join("records")).unwrap().any(|e| e
            .unwrap()
            .path()
            .extension()
            .is_some_and(|v| v == "input"))
    );
    // A delivered job is skipped before parsing, even if its old .job is unreadable.
    let job = fs::read_dir(root.path().join("records"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|v| v == "job"))
        .unwrap();
    fs::write(job, "historical record must not be read").unwrap();
    let mut restarted = fixture_with_bell(root.path(), "done", &bell.url);
    let running = restarted.source().spawn(sender);
    let mut unannounced = delivery.clone();
    unannounced.key = "effect:0:8".into();
    // A different adapter writes a job without this source's channel. Idle waits must not rescan it.
    adapter.execute(&unannounced).unwrap();
    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(1250)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(
        matches!(
            bell.requests.as_ref().unwrap().try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ),
        "idle source must not poll the bell"
    );
    restarted.execute(&unannounced).unwrap();
    bell.request().reply(1, serde_json::json!([]));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(10)).unwrap(),
        tl3_controller::cli::Input::Observation {
            observation: Observation::Checks {
                report: true,
                pass: true,
                ..
            },
            ..
        }
    ));
    drop(running);
    let calls = fs::read_to_string(root.path().join("queue/calls.jsonl")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.contains("--endings"))
            .count(),
        2
    );
}
fn finish(root: &Path, key: &str, mode: &str) {
    fs::write(root.join("queue/mode"), mode).unwrap();
    let finish = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pb.py"))
        .arg(root.join("queue"))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../prismabuild/tests/fixtures/queue"))
        .args(["--finish", key])
        .output()
        .unwrap();
    assert!(
        finish.status.success(),
        "{}",
        String::from_utf8_lossy(&finish.stderr)
    );
}
fn unreadable_ending_is_reported_once() {
    let root = tempfile::tempdir().unwrap();
    let first = run(root.path(), false);
    let mut second = first.clone();
    second.key = "effect:0:7".into();
    let mut adapter = fixture(root.path(), "done");
    adapter.execute(&first).unwrap();
    adapter.execute(&second).unwrap();
    let key = adapter.action_key(&first.key).unwrap().unwrap();
    fs::write(
        root.path().join(format!("queue/{key}.stdout")),
        "changed after PB verified it",
    )
    .unwrap();
    assert!(adapter.reconcile(&first).is_err());
    let (sender, receiver) = mpsc::channel();
    let running = adapter.source().spawn(sender.clone());
    let observations: Vec<_> = (0..2)
        .map(|_| receiver.recv_timeout(Duration::from_secs(10)).unwrap())
        .collect();
    drop(running);
    assert_eq!(
        observations
            .iter()
            .filter(|input| matches!(
                input,
                tl3_controller::cli::Input::Observation {
                    observation: Observation::Checks {
                        report: false,
                        pass: false,
                        ..
                    },
                    ..
                }
            ))
            .count(),
        1
    );
    assert!(!observed(completion(&mut adapter, &first)).0);
    let running = adapter.source().spawn(sender);
    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(1250)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    drop(running);
    let calls = fs::read_to_string(root.path().join("queue/calls.jsonl")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.contains("--endings"))
            .count(),
        3
    );
}
fn reconcile_never_submits() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut adapter = fixture(root.path(), "pending");
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Absent
    ));
    assert!(!root.path().join("queue/calls.jsonl").exists());
    adapter.execute(&delivery).unwrap();
    let key = adapter.action_key(&delivery.key).unwrap().unwrap();
    fs::remove_file(root.path().join(format!("queue/ready/{key}.json"))).unwrap();
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Pending
    ));
    assert_eq!(
        fs::read_to_string(root.path().join("queue/publications"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    adapter.execute(&delivery).unwrap();
    assert_eq!(adapter.action_key(&delivery.key).unwrap(), Some(key));
}
fn footprint_paths_and_merge_base() {
    for case in ["rename", "deletion", "space", "newline", "case", "diverged"] {
        let root = tempfile::tempdir().unwrap();
        let mut delivery = run(root.path(), false);
        let run = delivery.checks.as_mut().unwrap();
        let repo = &run.repository;
        git(repo, &["reset", "--hard", &run.base_sha]);
        fs::remove_file(repo.join("untracked.py")).unwrap();
        let expected: Vec<&str> = match case {
            "rename" => {
                git(repo, &["mv", "value.py", "renamed.py"]);
                vec!["renamed.py", "value.py"]
            }
            "deletion" => {
                git(repo, &["rm", "value.py"]);
                vec!["value.py"]
            }
            "space" => {
                fs::write(repo.join("with space.py"), "pass\n").unwrap();
                vec!["with space.py"]
            }
            "newline" => {
                fs::write(repo.join("with\nnewline.py"), "pass\n").unwrap();
                vec!["with\nnewline.py"]
            }
            "case" => {
                git(repo, &["mv", "value.py", "Value.py"]);
                vec!["Value.py", "value.py"]
            }
            "diverged" => {
                fs::write(repo.join("only-on-main.py"), "pass\n").unwrap();
                git(repo, &["add", "."]);
                git(repo, &["commit", "--quiet", "-m", "main advances"]);
                let common = run.base_sha.clone();
                run.base_sha = git(repo, &["rev-parse", "HEAD"]);
                git(repo, &["checkout", "--detach", &common]);
                fs::write(repo.join("value.py"), "def value(): return 2\n").unwrap();
                vec!["value.py"]
            }
            _ => unreachable!(),
        };
        git(repo, &["add", "."]);
        git(repo, &["commit", "--quiet", "-m", case]);
        delivery.head_sha = Some(git(repo, &["rev-parse", "HEAD"]));
        let mut adapter = fixture(root.path(), "done");
        adapter.execute(&delivery).unwrap();
        assert_eq!(
            observed(completion(&mut adapter, &delivery)).2,
            expected,
            "{case}"
        );
    }
}

struct Workers {
    fake: adapters::workers::Fake,
    head: String,
}
impl Executor for Workers {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        let mut completion = self.fake.execute(delivery)?;
        for observation in &mut completion.observations {
            match observation {
                Observation::Made { sha, .. } => *sha = self.head.clone(),
                Observation::Plan { paths, .. } => *paths = vec!["value.py".into()],
                _ => {}
            }
        }
        Ok(completion)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.fake.reconcile(delivery)
    }
}
fn through_controller(settings: &tl3_config::Store, outside: bool, cancel: bool, recover: bool) {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), outside);
    let adapter = fixture(root.path(), "pending");
    if recover {
        fs::write(root.path().join("queue/fail_publish"), "once").unwrap();
    }
    let source = adapter.source();
    let fakes = root.path().join("fakes");
    let mut registry = adapters::simulation(&fakes).unwrap();
    registry.register(Upstream::PrismaBuild, adapter);
    registry.register(
        Upstream::Workers,
        Workers {
            fake: adapters::workers::Fake::new(&fakes),
            head: delivery.head_sha.clone().unwrap(),
        },
    );
    let mut controller = Controller::open(
        tl3_store::Store::open(settings).unwrap(),
        registry,
        Bootstrap {
            repos: vec![repository()],
            rob: 1,
            slots: 1,
            pool: 1,
        },
    )
    .unwrap();
    controller
        .intake(LabelledIssue {
            delivery: "pb-fixture".into(),
            owner: "RobTand".into(),
            repo: "tl3-sandbox".into(),
            installation: 1,
            number: 42,
            title: "PrismaBuild fixture".into(),
            actor: 1,
            label: "tl3".into(),
        })
        .unwrap();
    for _ in 0..100 {
        if !controller.drive_once().unwrap() {
            break;
        }
    }
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Checking);
    if cancel {
        let pending = controller.snapshot().unwrap().pending_effects;
        let effect = pending
            .iter()
            .find(|effect| {
                serde_json::from_value::<Delivery>(effect.payload.clone()).is_ok_and(|d| {
                    matches!(
                        d.output,
                        Out::Effect {
                            kind: EffectKind::SubmitChecks,
                            ..
                        }
                    )
                })
            })
            .unwrap();
        controller
            .observe(
                "stop",
                Observation::Kernel(Event::Command {
                    node: NodeId(0),
                    id: 1,
                    command: Control::Stop,
                }),
            )
            .unwrap();
        controller
            .observe(
                "late-check-plan",
                Observation::CheckPlan {
                    node: NodeId(0),
                    sha: delivery.head_sha.clone().unwrap(),
                    run: delivery.checks.clone().unwrap(),
                },
            )
            .unwrap();
        controller.drive_once().unwrap();
        controller.drive_once().unwrap();
        controller.drive_once().unwrap();
        assert_eq!(
            tl3_store::Store::open(settings)
                .unwrap()
                .effect(&effect.key)
                .unwrap()
                .unwrap()
                .state,
            tl3_store::EffectState::Cancelling
        );
        controller.drive_once().unwrap();
        assert_eq!(
            tl3_store::Store::open(settings)
                .unwrap()
                .effect(&effect.key)
                .unwrap()
                .unwrap()
                .state,
            tl3_store::EffectState::Cancelled
        );
        assert!(
            !root.path().join("queue/calls.jsonl").exists(),
            "cancellation must never seal or publish"
        );
        return;
    }
    controller
        .observe(
            "check-plan",
            Observation::CheckPlan {
                node: NodeId(0),
                sha: delivery.head_sha.clone().unwrap(),
                run: delivery.checks.clone().unwrap(),
            },
        )
        .unwrap();
    controller
        .observe(
            "stale-head-plan",
            Observation::CheckPlan {
                node: NodeId(0),
                sha: "a".repeat(40),
                run: CheckRun {
                    repository: root.path().join("absent-old-checkout"),
                    ..delivery.checks.clone().unwrap()
                },
            },
        )
        .unwrap();
    for _ in 0..100 {
        if !controller.drive_once().unwrap() {
            break;
        }
    }
    if recover {
        assert!(!root.path().join("queue/publications").exists());
        assert!(
            controller
                .snapshot()
                .unwrap()
                .pending_effects
                .iter()
                .any(|e| {
                    e.kind == "PrismaBuild" && e.state == tl3_store::EffectState::Uncertain
                })
        );
        fs::remove_file(root.path().join("queue/fail_publish")).unwrap();
        controller.retry_held_effects();
        controller
            .observe(
                "recovered-tool",
                Observation::CheckPlan {
                    node: NodeId(0),
                    sha: delivery.head_sha.clone().unwrap(),
                    run: delivery.checks.clone().unwrap(),
                },
            )
            .unwrap();
        for _ in 0..100 {
            if !controller.drive_once().unwrap() {
                break;
            }
        }
        assert!(
            root.path().join("queue/publications").exists(),
            "the controller must publish the saved seal after PrismaBuild recovers"
        );
        let calls = fs::read_to_string(root.path().join("queue/calls.jsonl")).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.contains("--seal-only"))
                .count(),
            1
        );
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.contains("--publish-sealed"))
                .count(),
            2
        );
    }
    let key = fs::read_to_string(root.path().join("queue/publications"))
        .unwrap()
        .trim()
        .to_owned();
    assert_eq!(key.lines().count(), 1, "publish exactly one action");
    finish(root.path(), &key, "done");
    let (sender, receiver) = mpsc::channel();
    let running = source.spawn(sender);
    match receiver.recv().unwrap() {
        tl3_controller::cli::Input::Observation { key, observation } => {
            controller.observe(&key, observation).unwrap()
        }
        _ => panic!("source observation"),
    }
    controller.drive_once().unwrap();
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        if outside {
            Phase::Making
        } else {
            Phase::Reviewing
        }
    );
    let paths: BTreeSet<_> = controller
        .snapshot()
        .unwrap()
        .interned
        .into_iter()
        .filter(|i| i.kind == tl3_store::InternKind::Path)
        .map(|i| i.value)
        .collect();
    assert!(paths.contains("value.py"));
    assert_eq!(paths.contains("outside.py"), outside);
    drop(running);
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--kill-child") {
        let root = Path::new(&args[2]);
        let delivery: Delivery =
            serde_json::from_slice(&fs::read(root.join("delivery.json")).unwrap()).unwrap();
        fixture(root, "pending").execute(&delivery).unwrap();
        println!("published");
        use std::io::Write;
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::park();
        }
    }
    for (name, test) in [
        ("recorded_endings", recorded_endings as fn()),
        ("pending_and_withdrawal", pending_and_withdrawal),
        ("unknown_outcomes", unknown_outcomes),
        ("SIGKILL_reuses_seal", crash_reuses_seal),
        (
            "source_thread_and_restart",
            source_thread_delivers_once_and_restarts_idle,
        ),
        ("unreadable_ending_once", unreadable_ending_is_reported_once),
        ("reconcile_never_submits", reconcile_never_submits),
        (
            "footprint_paths_and_merge_base",
            footprint_paths_and_merge_base,
        ),
    ] {
        test();
        println!("test {name} ... ok");
    }
    let mut database = support::Database::start();
    through_controller(&database.create("pb_pass"), false, false, false);
    println!("test recorded_records_reach_kernel ... ok");
    through_controller(&database.create("pb_footprint"), true, false, false);
    println!("test outside_footprint_fails_round ... ok");
    through_controller(&database.create("pb_cancel"), false, true, false);
    println!("test cancelling_unbound_checks_starts_no_work ... ok");
    through_controller(&database.create("pb_recover"), false, false, true);
    println!("test controller_recovers_failed_publication ... ok");
    database.stop();
    println!("11 PrismaBuild adapter scenarios passed; no nested submissions; no skips");
}
