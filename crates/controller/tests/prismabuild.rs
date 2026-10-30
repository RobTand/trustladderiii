#[path = "../../store/tests/support/mod.rs"]
mod support;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
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
use tl3_kernel::{EffectKind, NodeId, Out, Phase};
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
        bell_url: "http://127.0.0.1:9/".into(),
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
        ("withdrawn", (true, false)),
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
fn source_retries_one_bad_key_without_hiding_another() {
    let root = tempfile::tempdir().unwrap();
    let first = run(root.path(), false);
    let mut second = first.clone();
    second.key = "effect:0:7".into();
    let mut adapter = fixture(root.path(), "done");
    adapter.execute(&first).unwrap();
    adapter.execute(&second).unwrap();
    let first_key = adapter.action_key(&first.key).unwrap().unwrap();
    let second_key = adapter.action_key(&second.key).unwrap().unwrap();
    let record = root.path().join(format!("queue/done/{first_key}.json"));
    let valid = fs::read(&record).unwrap();
    fs::write(&record, "malformed").unwrap();
    let mut source = adapter.source();
    let (sender, receiver) = mpsc::channel();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime
            .block_on(source.poll(&sender, Duration::from_millis(20)))
            .unwrap(),
        1
    );
    assert!(
        matches!(receiver.recv().unwrap(), tl3_controller::cli::Input::Observation {
        observation: Observation::Checks { action_key, .. }, ..
    } if action_key == second_key.as_str())
    );
    fs::write(&record, valid).unwrap();
    assert_eq!(
        runtime
            .block_on(source.poll(&sender, Duration::from_millis(20)))
            .unwrap(),
        1
    );
    assert!(
        matches!(receiver.recv().unwrap(), tl3_controller::cli::Input::Observation {
        observation: Observation::Checks { action_key, .. }, ..
    } if action_key == first_key.as_str())
    );
    let log = root.path().join(format!("queue/{first_key}.stdout"));
    fs::write(log, "changed after PB verified it").unwrap();
    assert!(adapter.reconcile(&first).is_err());
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
fn through_controller(settings: &tl3_config::Store, outside: bool) {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), outside);
    let adapter = fixture(root.path(), "pending");
    let mut source = adapter.source();
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
            actor: 1,
            label: "tl3".into(),
        })
        .unwrap();
    controller
        .observe(
            "check-plan",
            Observation::CheckPlan {
                node: NodeId(0),
                run: delivery.checks.unwrap(),
            },
        )
        .unwrap();
    for _ in 0..100 {
        if !controller.drive_once().unwrap() {
            break;
        }
    }
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Checking);
    let key = fs::read_to_string(root.path().join("queue/publications"))
        .unwrap()
        .trim()
        .to_owned();
    fs::write(root.path().join("queue/mode"), "done").unwrap();
    let finish = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pb.py"))
        .arg(root.path().join("queue"))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../prismabuild/tests/fixtures/queue"))
        .args(["--finish", &key])
        .output()
        .unwrap();
    assert!(
        finish.status.success(),
        "{}",
        String::from_utf8_lossy(&finish.stderr)
    );
    let (sender, receiver) = mpsc::channel();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime
            .block_on(source.poll(&sender, Duration::from_millis(20)))
            .unwrap(),
        1
    );
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
    assert_eq!(
        runtime
            .block_on(source.poll(&sender, Duration::from_millis(20)))
            .unwrap(),
        0
    );
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
            "per_key_source_recovery",
            source_retries_one_bad_key_without_hiding_another,
        ),
    ] {
        test();
        println!("test {name} ... ok");
    }
    let mut database = support::Database::start();
    through_controller(&database.create("pb_pass"), false);
    println!("test recorded_records_reach_kernel ... ok");
    through_controller(&database.create("pb_footprint"), true);
    println!("test outside_footprint_fails_round ... ok");
    database.stop();
    println!("7 PrismaBuild adapter scenarios passed; no nested submissions; no skips");
}
