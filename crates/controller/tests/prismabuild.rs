#[path = "fixtures/bell/mod.rs"]
mod bell;
use bell::Bell;
mod support;
use std::{
    collections::BTreeSet,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository,
    adapters::{
        self,
        prismabuild::{Adapter, Check, CheckRun, Kind, Settings},
    },
    executor::Output as Out,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation, Upstream},
};
use tl3_kernel::{Command as Control, EffectKind, Event, NodeId, Phase};
use tl3_prismabuild::PrismaBuild;
mod prismabuild_fixture;

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
        replanned: false,
        marker_key: None,
        note: None,
        steering: None,
        request_text: None,
        question_context: None,
        question: None,
        job_generation: Some(4),
        checks: Some(CheckRun {
            repository: repo,
            base_sha: tl3_worktrees::CommitId::new(base).unwrap(),

            reviewed_sha: None,
            reproducing_unavailable: None,
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
    let (client, settings) = fixture_settings(root, mode, bell_url);
    Adapter::new(client, settings).unwrap()
}
fn fixture_settings(root: &Path, mode: &str, bell_url: &str) -> (PrismaBuild, Settings) {
    prismabuild_fixture::settings(root, mode, bell_url)
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
fn bounded_failure_details() {
    let root = tempfile::tempdir().unwrap();
    let mut delivery = run(root.path(), false);
    delivery.checks.as_mut().unwrap().plan[0].tests = (0..25)
        .map(|i| format!("test_value.py::test_{i}"))
        .collect();
    let mut adapter = fixture(root.path(), "check_fail");
    fs::write(root.path().join("queue/failure_message"), "é".repeat(2000)).unwrap();
    adapter.execute(&delivery).unwrap();
    let result = completion(&mut adapter, &delivery);
    match &result.observations[0] {
        Observation::Checks {
            failed_tests,
            results,
            report,
            pass,
            ..
        } => {
            assert!(*report && !*pass);
            assert!(
                results
                    .iter()
                    .flat_map(|result| result["tests"].as_array().unwrap())
                    .all(|test| test["message"] == "é".repeat(1000)),
                "reviewer check messages must be clipped"
            );
            assert_eq!(failed_tests.len(), 20);
            assert_eq!(failed_tests[0].test, "test_value.py::test_0");
            assert!(
                failed_tests
                    .iter()
                    .all(|test| test.message == "é".repeat(1000))
            );
        }
        _ => panic!("check observation required"),
    }
    let mut legacy = serde_json::to_value(&result.observations[0]).unwrap();
    legacy["Checks"]
        .as_object_mut()
        .unwrap()
        .remove("failed_tests");
    assert!(
        matches!(serde_json::from_value::<Observation>(legacy).unwrap(), Observation::Checks { failed_tests, .. } if failed_tests.is_empty())
    );
}

fn reproducing_check_plan_and_verdict() {
    let root = tempfile::tempdir().unwrap();
    let mut delivery = run(root.path(), false);
    let mut value = serde_json::to_value(delivery.checks.as_ref().unwrap()).unwrap();
    let reviewed = delivery.checks.as_ref().unwrap().base_sha.clone();
    value["reviewed_sha"] = serde_json::json!(reviewed);
    value["plan"][0]["kind"] = serde_json::json!("reproducing");
    let parsed = serde_json::from_value::<CheckRun>(value);
    assert!(
        parsed.is_ok(),
        "the check plan must carry reproducing checks"
    );
    delivery.checks = Some(parsed.unwrap());
    for (mode, expected) in [("done", true), ("check_fail", false)] {
        let trial = tempfile::tempdir().unwrap();
        let mut adapter = fixture(trial.path(), mode);
        adapter.execute(&delivery).unwrap();
        let requests = fs::read_dir(trial.path().join("queue")).unwrap();
        let path = requests
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "request")
            })
            .unwrap();
        let request: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(
            request["reviewed"],
            reviewed.as_str(),
            "freeze the exact reviewed commit"
        );
        let (report, pass, _) = observed(completion(&mut adapter, &delivery));
        assert!(report);
        assert_eq!(pass, expected, "a failed reproducer is a failed round");
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
    let key = adapter.action_key(&delivery).unwrap().unwrap();
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
    use rustix::{
        event::{PollFd, PollFlags, Timespec, poll},
        process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal},
    };
    use std::os::unix::net::UnixListener;
    for stage in ["before", "after"] {
        let root = tempfile::tempdir().unwrap();
        let delivery = run(root.path(), false);
        fs::write(
            root.path().join("delivery.json"),
            serde_json::to_vec(&delivery).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(root.path().join("queue")).unwrap();
        let socket = root.path().join("queue/publish.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        fs::write(root.path().join("queue/publish_stage"), stage).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--kill-child")
            .arg(root.path())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let connection = loop {
            match listener.accept() {
                Ok((connection, _)) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("publisher gate: {error}"),
            }
            assert!(
                std::time::Instant::now() < deadline,
                "fixture never reached publication"
            );
            assert!(
                child.try_wait().unwrap().is_none(),
                "adapter child exited before publication"
            );
            thread::sleep(Duration::from_millis(10));
        };
        connection
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut publisher = String::new();
        let mut connection = BufReader::new(connection);
        connection.read_line(&mut publisher).unwrap();
        let pid = Pid::from_raw(publisher.trim().parse().unwrap()).unwrap();
        let publisher = pidfd_open(pid, PidfdFlags::empty()).unwrap();
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());
        let mut events = [PollFd::new(&publisher, PollFlags::IN)];
        let ended = poll(
            &mut events,
            Some(&Timespec {
                tv_sec: 10,
                tv_nsec: 0,
            }),
        )
        .unwrap();
        if ended == 0 {
            pidfd_send_signal(&publisher, Signal::KILL).unwrap();
        }
        assert_eq!(ended, 1, "publisher must die with its controller");
        assert!(events[0].revents().contains(PollFlags::IN));
        fs::remove_file(root.path().join("queue/publish_stage")).unwrap();
        let mut adapter = fixture(root.path(), "pending");
        let key = adapter.action_key(&delivery).unwrap().unwrap();
        assert!(
            matches!(
                adapter.reconcile(&delivery).unwrap(),
                Reconciliation::Absent
            ) == (stage == "before")
        );
        // Recovery cannot depend on the old checkout, and must use the saved seal.
        fs::remove_dir_all(&delivery.checks.as_ref().unwrap().repository).unwrap();
        adapter.execute(&delivery).unwrap();
        assert_eq!(adapter.action_key(&delivery).unwrap(), Some(key.clone()));
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
        assert_eq!(
            calls
                .iter()
                .filter(|c| c.contains(&"--publish-sealed".to_owned()))
                .count(),
            if stage == "before" { 2 } else { 1 }
        );
        // The recovered action is watched, whether it landed before death or was republished.
        let (sender, receiver) = mpsc::channel();
        finish(root.path(), key.as_str(), "done");
        let running = adapter.source().spawn(sender);
        assert_passing_input(receiver.recv_timeout(Duration::from_secs(10)).unwrap());
        drop(running);
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
    let key = adapter.action_key(&delivery).unwrap().unwrap();
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
        fs::read_dir(root.path().join("records/0"))
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
        !fs::read_dir(root.path().join("records/0"))
            .unwrap()
            .any(|e| e.unwrap().path().extension().is_some_and(|v| v == "input"))
    );
    // A delivered job is skipped before parsing, even if its old .job is unreadable.
    let job = fs::read_dir(root.path().join("records/0"))
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
fn invalid_evidence_is_reported_once() {
    for damage in ["digest", "receipt", "oversized"] {
        let root = tempfile::tempdir().unwrap();
        let first = run(root.path(), false);
        let mut second = first.clone();
        second.key = "effect:0:7".into();
        let mut adapter = fixture(root.path(), "done");
        adapter.execute(&first).unwrap();
        adapter.execute(&second).unwrap();
        let key = adapter.action_key(&first).unwrap().unwrap();
        if damage == "digest" {
            fs::write(
                root.path().join(format!("queue/{key}.stdout")),
                "changed after PB verified it",
            )
            .unwrap();
        } else {
            let path = root.path().join(format!("queue/{key}.ending"));
            let mut ending: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            let evidence = &mut ending["endings"][0]["result"];
            if damage == "receipt" {
                evidence["verified"] = false.into();
            } else {
                evidence["stdout"]["bytes"] = (8 * 1024 * 1024 + 1).into();
            }
            fs::write(path, serde_json::to_vec(&ending).unwrap()).unwrap();
        }
        assert!(!observed(completion(&mut adapter, &first)).0);
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
    let key = adapter.action_key(&delivery).unwrap().unwrap();
    fs::remove_file(root.path().join(format!("queue/ready/{key}.json"))).unwrap();
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Absent
    ));
    assert_eq!(
        fs::read_to_string(root.path().join("queue/publications"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    adapter.execute(&delivery).unwrap();
    assert_eq!(adapter.action_key(&delivery).unwrap(), Some(key));
}
fn footprint_paths_and_merge_base() {
    for case in ["rename", "deletion", "space", "newline", "case", "diverged"] {
        let root = tempfile::tempdir().unwrap();
        let mut delivery = run(root.path(), false);
        let run = delivery.checks.as_mut().unwrap();
        let repo = &run.repository;
        git(repo, &["reset", "--hard", run.base_sha.as_str()]);
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
                run.base_sha =
                    tl3_worktrees::CommitId::new(git(repo, &["rev-parse", "HEAD"])).unwrap();
                git(repo, &["checkout", "--detach", common.as_str()]);
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

fn record_path(root: &Path, key: &str, extension: &str) -> PathBuf {
    let encoded: String = key.bytes().map(|b| format!("{b:02x}")).collect();
    let node = key.split(':').nth(1).unwrap();
    root.join("records")
        .join(node)
        .join(format!("{encoded}.{extension}"))
}

fn tool_errors_wait_for_news() {
    for spawn_failure in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let delivery = run(root.path(), false);
        let bell = Bell::new();
        let (client, mut settings) = fixture_settings(root.path(), "done", &bell.url);
        let program = root.path().join("result-python");
        if spawn_failure {
            settings.result_tool.program = program.clone();
        } else {
            fs::write(root.path().join("queue/garbled_ending"), "true").unwrap();
        }
        let mut adapter = Adapter::new(client, settings).unwrap();
        adapter.execute(&delivery).unwrap();
        let key = adapter.action_key(&delivery).unwrap().unwrap();
        let (sender, receiver) = mpsc::channel();
        let running = adapter.source().spawn(sender.clone());
        bell.request().reply(0, serde_json::json!([]));
        let waiting = bell.request();
        assert!(waiting.line.contains("/v1/wait"));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(1250)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(!record_path(root.path(), &delivery.key, "done").exists());
        let calls = fs::read_to_string(root.path().join("queue/calls.jsonl")).unwrap();
        assert_eq!(
            calls.lines().filter(|l| l.contains("--endings")).count(),
            usize::from(!spawn_failure),
            "no tool retry without news"
        );
        if spawn_failure {
            drop(running);
            drop(waiting);
            std::os::unix::fs::symlink("/usr/bin/python3", program).unwrap();
            let running = adapter.source().spawn(sender);
            bell.request().reply(0, serde_json::json!([]));
            assert_passing_input(receiver.recv_timeout(Duration::from_secs(10)).unwrap());
            drop(running);
        } else {
            fs::remove_file(root.path().join("queue/garbled_ending")).unwrap();
            waiting.reply(1, serde_json::json!([{"seq":1,"dir":"done","key":key}]));
            assert_passing_input(receiver.recv_timeout(Duration::from_secs(10)).unwrap());
            drop(running);
        }
        assert!(record_path(root.path(), &delivery.key, "done").exists());
    }
}

fn assert_passing_input(input: tl3_controller::cli::Input) {
    assert!(matches!(
        input,
        tl3_controller::cli::Input::Observation {
            observation: Observation::Checks {
                report: true,
                pass: true,
                ..
            },
            ..
        }
    ));
}

fn source_skips_bad_jobs_and_surfaces_failure() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut bad = delivery.clone();
    bad.key = "effect:0:7".into();
    let mut adapter = fixture(root.path(), "done");
    let source = adapter.source();
    adapter.execute(&bad).unwrap();
    fs::write(record_path(root.path(), &bad.key, "job"), "broken record").unwrap();
    adapter.execute(&delivery).unwrap();
    let (sender, receiver) = mpsc::channel();
    // Both rebuild and queued key registration encounter the bad record.
    let running = source.spawn(sender);
    assert_passing_input(receiver.recv_timeout(Duration::from_secs(10)).unwrap());
    drop(running);
    assert!(!record_path(root.path(), &bad.key, "done").exists());
    let mut withdrawal = bad.clone();
    withdrawal.output = Out::Effect {
        node: NodeId(0),
        seq: 8,
        kind: EffectKind::WithdrawJob,
        head: None,
    };
    assert!(
        adapter.execute(&withdrawal).is_err(),
        "a corrupt active binding must not confirm withdrawal"
    );
    assert!(adapter.reconcile(&withdrawal).is_err());

    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let bell = Bell::new();
    let mut adapter = fixture_with_bell(root.path(), "pending", &bell.url);
    adapter.execute(&delivery).unwrap();
    let key = adapter.action_key(&delivery).unwrap().unwrap();
    let (sender, receiver) = mpsc::channel();
    let running = adapter.source().spawn(sender);
    bell.request().reply(0, serde_json::json!([]));
    let waiting = bell.request();
    // Introduce the write failure only after rebuild, so this job is already active.
    fs::create_dir(record_path(root.path(), &delivery.key, "done")).unwrap();
    finish(root.path(), key.as_str(), "done");
    waiting.reply(1, serde_json::json!([{"seq":1,"dir":"done","key":key}]));
    assert_passing_input(receiver.recv_timeout(Duration::from_secs(10)).unwrap());
    assert!(
        running.join().is_err(),
        "the source supervisor must see a failed marker write"
    );
    let source = adapter.source();
    fs::rename(
        root.path().join("records"),
        root.path().join("moved-records"),
    )
    .unwrap();
    let (sender, _) = mpsc::channel();
    assert!(
        thread::spawn(move || tl3_controller::ingress::Source::run(source, sender))
            .join()
            .unwrap()
            .is_err(),
        "a failed registered source must reach the ingress supervisor"
    );
}

fn bell_outage_does_not_retry_result_tools() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut adapter = fixture(root.path(), "done");
    adapter.execute(&delivery).unwrap();
    fs::write(root.path().join("queue/garbled_ending"), "true").unwrap();
    let mut source = adapter.source();
    let (sender, receiver) = mpsc::channel();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for _ in 0..3 {
        assert_eq!(
            runtime
                .block_on(source.poll(&sender, Duration::from_secs(1)))
                .unwrap(),
            0
        );
    }
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    let calls = fs::read_to_string(root.path().join("queue/calls.jsonl")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.contains("--endings"))
            .count(),
        1,
        "bell transport backoff must not become a result-tool timer"
    );
}

fn absent_withdrawal_retires_source_watch() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let bell = Bell::new();
    let mut adapter = fixture_with_bell(root.path(), "pending", &bell.url);
    fs::write(root.path().join("queue/fail_publish"), "true").unwrap();
    assert!(adapter.execute(&delivery).is_err());
    let (sender, receiver) = mpsc::channel();
    let running = adapter.source().spawn(sender.clone());
    bell.request().reply(0, serde_json::json!([]));
    let waiting = bell.request();
    let mut withdrawal = delivery.clone();
    withdrawal.key = "effect:0:6".into();
    withdrawal.output = Out::Effect {
        node: NodeId(0),
        seq: 6,
        kind: EffectKind::WithdrawJob,
        head: None,
    };
    adapter.execute(&withdrawal).unwrap();
    let done: Completion =
        serde_json::from_slice(&fs::read(record_path(root.path(), &delivery.key, "done")).unwrap())
            .unwrap();
    assert!(done.observations.is_empty());
    // Reply after cancellation. A forgotten watch must not open another bell wait.
    waiting.reply(1, serde_json::json!([]));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(1250)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(matches!(
        bell.requests.as_ref().unwrap().try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    drop(running);
    let running = adapter.source().spawn(sender);
    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(1250)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(matches!(
        bell.requests.as_ref().unwrap().try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    drop(running);
    assert!(!root.path().join("queue/publications").exists());
}

fn completed_records_retire_only_when_superseded() {
    let root = tempfile::tempdir().unwrap();
    let mut delivery = run(root.path(), false);
    let mut adapter = fixture(root.path(), "done");
    let (sender, receiver) = mpsc::channel();
    let mut previous: Option<Delivery> = None;
    for generation in 4..8 {
        delivery.key = format!("effect:0:{generation}");
        delivery.generation = generation;
        adapter.execute(&delivery).unwrap();
        if let Some(previous) = &previous {
            assert!(
                record_path(root.path(), &previous.key, "job").exists(),
                "retain recovery records until the newer ending is delivered"
            );
        }
        assert!(matches!(
            adapter.reconcile(&delivery).unwrap(),
            Reconciliation::Found(_)
        ));
        let running = adapter.source().spawn(sender.clone());
        assert_passing_input(receiver.recv_timeout(Duration::from_secs(10)).unwrap());
        drop(running);
        if let Some(previous) = previous {
            for ext in ["job", "done"] {
                assert!(!record_path(root.path(), &previous.key, ext).exists());
            }
        }
        assert!(record_path(root.path(), &delivery.key, "job").exists());
        assert!(record_path(root.path(), &delivery.key, "done").exists());
        previous = Some(delivery.clone());
    }
    // Supersession and teardown must never list another node's files.
    // A regular file at another node's directory makes accidental global scans fail.
    fs::write(root.path().join("records/999"), "unrelated node").unwrap();
    adapter.retire_node(NodeId(0)).unwrap();
    adapter.retire_node(NodeId(0)).unwrap();
    assert!(!root.path().join("records/0").exists());
    assert_eq!(
        fs::read_to_string(root.path().join("records/999")).unwrap(),
        "unrelated node"
    );
    assert!(adapter.action_key(&delivery).unwrap().is_none());
    // Restore a completed current record for the withdrawal assertion below.
    adapter.execute(&delivery).unwrap();
    let running = adapter.source().spawn(sender.clone());
    assert_passing_input(receiver.recv_timeout(Duration::from_secs(10)).unwrap());
    drop(running);
    // Delivered records are skipped before parsing when finding withdrawal targets.
    fs::write(
        record_path(root.path(), &delivery.key, "job"),
        "do not parse",
    )
    .unwrap();
    let mut withdraw = delivery.clone();
    withdraw.output = Out::Effect {
        node: NodeId(0),
        seq: 20,
        kind: EffectKind::WithdrawJob,
        head: None,
    };
    withdraw.job_generation = Some(delivery.generation);
    adapter.execute(&withdraw).unwrap();

    let root = tempfile::tempdir().unwrap();
    let first = run(root.path(), false);
    let mut adapter = fixture(root.path(), "pending");
    adapter.execute(&first).unwrap();
    let mut later = first.clone();
    later.key = "effect:0:8".into();
    later.generation += 1;
    adapter.execute(&later).unwrap();
    assert!(
        record_path(root.path(), &first.key, "job").exists(),
        "retain undelivered work"
    );
    let mut other = later.clone();
    other.output = Out::Effect {
        node: NodeId(1),
        seq: 8,
        kind: EffectKind::SubmitChecks,
        head: None,
    };
    other.key = "effect:1:8".into();
    fs::write(record_path(root.path(), &first.key, "done"), b"{}").unwrap();
    adapter.execute(&other).unwrap();
    assert!(
        record_path(root.path(), &first.key, "job").exists(),
        "retain another node's completion"
    );
    fs::remove_file(record_path(root.path(), &first.key, "done")).unwrap();
    let first_key = adapter.action_key(&first).unwrap().unwrap();
    fs::remove_file(root.path().join(format!("queue/ready/{first_key}.json"))).unwrap();
    let key = adapter.action_key(&later).unwrap().unwrap();
    finish(root.path(), key.as_str(), "done");
    let (sender, receiver) = mpsc::channel();
    let running = adapter.source().spawn(sender);
    assert_passing_input(receiver.recv_timeout(Duration::from_secs(10)).unwrap());
    drop(running);
    assert!(
        !record_path(root.path(), &first.key, "job").exists(),
        "newer delivered checks also retire a never-published cancelled seal"
    );
    assert!(record_path(root.path(), &other.key, "job").exists());
    let other_path = record_path(root.path(), &other.key, "job");
    let other_bytes = fs::read(&other_path).unwrap();
    fs::write(&other_path, "another node must not be parsed").unwrap();
    let mut withdrawal = later.clone();
    withdrawal.output = Out::Effect {
        node: later.node(),
        seq: 99,
        kind: EffectKind::WithdrawJob,
        head: None,
    };
    withdrawal.job_generation = Some(later.generation);
    adapter.execute(&withdrawal).unwrap();
    fs::write(other_path, other_bytes).unwrap();
    adapter.retire_node(first.node()).unwrap();
    assert!(!root.path().join("records/0").exists());
    assert!(record_path(root.path(), &other.key, "job").exists());
    drop(adapter);
    let adapter = fixture(root.path(), "pending");
    assert!(adapter.action_key(&first).unwrap().is_none());
    assert!(adapter.action_key(&other).unwrap().is_some());
}

fn retirement_refuses_active_or_unreadable_jobs() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut adapter = fixture(root.path(), "pending");
    adapter.execute(&delivery).unwrap();
    let key = adapter.action_key(&delivery).unwrap().unwrap();
    assert!(adapter.retire_node(delivery.node()).is_err());
    let queue = root.path().join("queue");
    fs::rename(
        queue.join(format!("ready/{key}.json")),
        queue.join(format!("claimed/{key}.json")),
    )
    .unwrap();
    assert!(adapter.retire_node(delivery.node()).is_err());
    assert!(adapter.action_key(&delivery).unwrap().is_some());
    let path = record_path(root.path(), &delivery.key, "job");
    let binding = fs::read(&path).unwrap();
    fs::write(&path, "invalid binding").unwrap();
    assert!(adapter.retire_node(delivery.node()).is_err());
    assert!(path.exists());
    fs::write(&path, binding).unwrap();
    fs::remove_file(queue.join(format!("claimed/{key}.json"))).unwrap();
    adapter.retire_node(delivery.node()).unwrap();
    adapter.retire_node(delivery.node()).unwrap();
    assert!(!root.path().join("records/0").exists());
}

fn retirement_forgets_an_active_source() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let bell = Bell::new();
    let mut adapter = fixture_with_bell(root.path(), "pending", &bell.url);
    fs::write(root.path().join("queue/fail_publish"), "true").unwrap();
    assert!(adapter.execute(&delivery).is_err());
    let (sender, receiver) = mpsc::channel();
    let running = adapter.source().spawn(sender.clone());
    bell.request().reply(0, serde_json::json!([]));
    let waiting = bell.request();
    // This seal is absent and cancelled, so its request can close. The source still
    // has a cached watch until teardown's notification is consumed.
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Absent
    ));
    adapter.retire_node(delivery.node()).unwrap();
    waiting.reply(1, serde_json::json!([]));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(1250)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(matches!(
        bell.requests.as_ref().unwrap().try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    drop(running);
    let running = adapter.source().spawn(sender);
    assert!(!root.path().join("records/0").exists());
    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(1250)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(matches!(
        bell.requests.as_ref().unwrap().try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    drop(running);
    assert!(!root.path().join("queue/publications").exists());
}

fn cancellation_keeps_an_inflight_ending_from_replacing_its_marker() {
    use std::os::unix::net::UnixListener;
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut adapter = fixture(root.path(), "done");
    adapter.execute(&delivery).unwrap();
    let key = adapter.action_key(&delivery).unwrap().unwrap();
    let listener = UnixListener::bind(root.path().join("queue/publish.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    fs::write(root.path().join("queue/publish_stage"), "ending").unwrap();
    let (sender, receiver) = mpsc::channel();
    let running = adapter.source().spawn(sender);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let connection = loop {
        match listener.accept() {
            Ok((connection, _)) => break connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("ending gate: {error}"),
        }
        assert!(
            std::time::Instant::now() < deadline,
            "source did not read ending"
        );
        thread::sleep(Duration::from_millis(10));
    };
    connection
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut connection = BufReader::new(connection);
    let mut ready = String::new();
    connection.read_line(&mut ready).unwrap();
    fs::remove_file(root.path().join(format!("queue/done/{key}.json"))).unwrap();
    let mut withdrawal = delivery.clone();
    withdrawal.output = Out::Effect {
        node: delivery.node(),
        seq: 99,
        kind: EffectKind::WithdrawJob,
        head: None,
    };
    withdrawal.job_generation = Some(delivery.generation);
    assert!(matches!(
        adapter.reconcile(&withdrawal).unwrap(),
        Reconciliation::Found(_)
    ));
    fs::remove_file(root.path().join("queue/publish_stage")).unwrap();
    connection.get_mut().write_all(b"!").unwrap();
    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(1250)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    drop(running);
    let completion: Completion =
        serde_json::from_slice(&fs::read(record_path(root.path(), &delivery.key, "done")).unwrap())
            .unwrap();
    assert!(
        completion.observations.is_empty(),
        "an obsolete source result must not replace cancellation's marker"
    );
}

fn endings_outlive_queue_rows() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut adapter = fixture(root.path(), "done");
    adapter.execute(&delivery).unwrap();
    let key = adapter.action_key(&delivery).unwrap().unwrap();
    fs::remove_file(root.path().join(format!("queue/done/{key}.json"))).unwrap();
    assert!(observed(completion(&mut adapter, &delivery)).1);
    let (sender, receiver) = mpsc::channel();
    let running = adapter.source().spawn(sender);
    assert_passing_input(receiver.recv_timeout(Duration::from_secs(10)).unwrap());
    drop(running);
    // An upgrade from flat records must retain both the binding and its done marker.
    for extension in ["job", "done"] {
        let path = record_path(root.path(), &delivery.key, extension);
        fs::rename(
            &path,
            root.path().join("records").join(path.file_name().unwrap()),
        )
        .unwrap();
    }
    drop(adapter);
    let mut adapter = fixture(root.path(), "done");
    assert_eq!(adapter.action_key(&delivery).unwrap(), Some(key));
    assert!(observed(completion(&mut adapter, &delivery)).1);
    let (sender, receiver) = mpsc::channel();
    let running = adapter.source().spawn(sender);
    assert!(receiver.try_recv().is_err());
    drop(running);
}

fn abandoned_inputs_are_removed() {
    let root = tempfile::tempdir().unwrap();
    let mut delivery = run(root.path(), false);
    let mut adapter = fixture(root.path(), "pending");
    delivery.checks.as_mut().unwrap().base_sha =
        tl3_worktrees::CommitId::new("a".repeat(40)).unwrap();
    assert!(adapter.execute(&delivery).is_err());
    let input = record_path(root.path(), &delivery.key, "input");
    assert!(input.exists(), "failed staging fixture left its input");
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Absent
    ));
    assert!(!input.exists(), "cancelled effects may never execute again");
    fs::create_dir_all(&input).unwrap();
    let orphan = root.path().join("records/retired.done");
    fs::write(&orphan, "retirement interrupted").unwrap();
    drop(adapter);
    let mut adapter = fixture(root.path(), "pending");
    assert!(!input.exists(), "startup removes an unbound input");
    assert!(!orphan.exists(), "startup finishes interrupted retirement");
    delivery.checks.as_mut().unwrap().base_sha = tl3_worktrees::CommitId::new(git(
        &delivery.checks.as_ref().unwrap().repository,
        &["rev-parse", "HEAD^"],
    ))
    .unwrap();
    adapter.execute(&delivery).unwrap();
    fs::create_dir_all(&input).unwrap();
    drop(adapter);
    let _adapter = fixture(root.path(), "pending");
    assert!(
        !input.exists(),
        "startup also removes an already sealed input"
    );
}

fn workers(fake: impl Executor + 'static, head: String) -> impl Executor {
    support::workers::Workers::new(None, fake, vec![], move |_, completion| {
        for observation in &mut completion.observations {
            match observation {
                Observation::Made { sha, .. } => *sha = head.clone(),
                Observation::Plan { paths, .. } => *paths = vec!["value.py".into()],
                _ => {}
            }
        }
    })
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
        workers(
            adapters::workers::Fake::new(&fakes),
            delivery.head_sha.clone().unwrap(),
        ),
    );
    let mut controller = Controller::open(
        tl3_store::Store::open(settings).unwrap(),
        registry,
        Bootstrap {
            operations: None,
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
    if cancel && !recover {
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
        if cancel {
            let effect = controller
                .snapshot()
                .unwrap()
                .pending_effects
                .into_iter()
                .find(|e| e.kind == "PrismaBuild")
                .unwrap();
            controller
                .observe(
                    "cancel-sealed",
                    Observation::Kernel(Event::Command {
                        node: NodeId(0),
                        id: 1,
                        command: Control::Stop,
                    }),
                )
                .unwrap();
            for _ in 0..100 {
                if !controller.drive_once().unwrap() {
                    break;
                }
            }
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
                !root.path().join("queue/publications").exists(),
                "cancelling a never-published seal must not publish it"
            );
            return;
        }
        controller.retry_held_effects(Upstream::PrismaBuild);
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
    if outside {
        let findings = tl3_store::Store::open(settings)
            .unwrap()
            .worker_document("0", "maker_findings")
            .unwrap()
            .unwrap();
        assert_eq!(
            findings[0]["outside_paths"],
            serde_json::json!(["outside.py"])
        );
        assert_eq!(
            findings[0]["goal_ids"],
            serde_json::json!([]),
            "missing maker goals must retain unbound evidence"
        );
    }
    drop(running);
}
fn retirement_continues_after_corrupt_node(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let mut controller = Controller::open(
        tl3_store::Store::open(settings).unwrap(),
        adapters::simulation(&root.path().join("fakes")).unwrap(),
        Bootstrap {
            repos: vec![repository()],
            rob: 1,
            slots: 1,
            pool: 1,
            operations: None,
        },
    )
    .unwrap();
    for node in 0..3 {
        controller
            .intake(LabelledIssue {
                delivery: format!("retirement-{node}"),
                owner: "RobTand".into(),
                repo: "tl3-sandbox".into(),
                installation: 1,
                number: 42 + node,
                title: "Retirement fixture".into(),
                actor: 1,
                label: "tl3".into(),
            })
            .unwrap();
        while controller.drive_once().unwrap() {}
    }
    assert!(
        controller
            .world()
            .unwrap()
            .nodes
            .iter()
            .all(|n| n.phase.is_closed())
    );
    let mut adapter = fixture(root.path(), "done");
    let records = root.path().join("records");
    for node in 0..3 {
        fs::create_dir_all(records.join(node.to_string())).unwrap();
    }
    // Corrupt the first candidate regardless of filesystem directory ordering.
    let bad = fs::read_dir(&records)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    fs::write(bad.join("corrupt.job"), "not JSON").unwrap();
    let mut store = tl3_store::Store::open(settings).unwrap();
    assert!(
        adapter.retire(&mut store).is_err(),
        "retirement must still report the corrupt job"
    );
    assert!(bad.exists(), "unreadable evidence must remain for recovery");
    let remaining: Vec<_> = fs::read_dir(&records)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    assert_eq!(
        remaining,
        vec![bad],
        "one corrupt node must not block other retirements"
    );
}

fn main() {
    if std::env::args().len() == 1
        || std::env::args().nth(1).as_deref() == Some("--router-regression")
    {
        let mut router = workers(support::executor_probe::Probe, "a".repeat(40));
        support::executor_probe::assert_retire_delivery(&mut router);
        if std::env::args().len() > 1 {
            return;
        }
    }

    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--r5") {
        let mut failures = 0;
        for (name, test) in [
            (
                "moving_wait_bell_is_quiet",
                moving_wait_bell_is_quiet as fn(),
            ),
            (
                "moving_job_reconcile_is_pending",
                moving_job_reconcile_is_pending,
            ),
        ] {
            let result = std::panic::catch_unwind(test);
            println!(
                "R5 {name}: {}",
                if result.is_ok() { "PASS" } else { "FAIL" }
            );
            failures += usize::from(result.is_err());
        }
        assert_eq!(failures, 0, "R5 adapter regressions");
        return;
    }
    if args.get(1).is_some_and(|arg| arg == "--r4-bad-waits") {
        source_skips_bad_waits();
        return;
    }
    if args.get(1).is_some_and(|arg| arg == "--r4-bell") {
        live_waits_do_not_look_on_bell_reset();
        return;
    }
    if args.get(1).is_some_and(|arg| arg == "--r3") {
        declared_wait_uses_bell();
        return;
    }
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
    if args
        .get(1)
        .is_some_and(|arg| arg == "--retirement-regression")
    {
        let mut database = support::Database::start();
        retirement_continues_after_corrupt_node(&database.create("pb_retirement"));
        database.stop();
        println!("test retirement_continues_after_corrupt_node ... ok");
        return;
    }
    for (name, test) in [
        (
            "moving_wait_bell_is_quiet",
            moving_wait_bell_is_quiet as fn(),
        ),
        (
            "moving_job_reconcile_is_pending",
            moving_job_reconcile_is_pending,
        ),
        (
            "reproducing_check_plan_and_verdict",
            reproducing_check_plan_and_verdict,
        ),
        ("source_skips_bad_waits", source_skips_bad_waits as fn()),
        (
            "live_waits_do_not_look_on_bell_reset",
            live_waits_do_not_look_on_bell_reset,
        ),
        ("declared_wait_uses_bell", declared_wait_uses_bell as fn()),
        ("recorded_endings", recorded_endings as fn()),
        (
            "retirement_refuses_active_or_unreadable_jobs",
            retirement_refuses_active_or_unreadable_jobs,
        ),
        ("bounded_failure_details", bounded_failure_details),
        ("pending_and_withdrawal", pending_and_withdrawal),
        ("unknown_outcomes", unknown_outcomes),
        ("SIGKILL_reuses_seal", crash_reuses_seal),
        (
            "source_thread_and_restart",
            source_thread_delivers_once_and_restarts_idle,
        ),
        ("invalid_evidence_once", invalid_evidence_is_reported_once),
        ("tool_errors_wait_for_news", tool_errors_wait_for_news),
        (
            "bell_outage_does_not_retry_result_tools",
            bell_outage_does_not_retry_result_tools,
        ),
        (
            "absent_withdrawal_retires_source_watch",
            absent_withdrawal_retires_source_watch,
        ),
        (
            "source_skips_bad_jobs_and_surfaces_failure",
            source_skips_bad_jobs_and_surfaces_failure,
        ),
        (
            "completed_records_retire_only_when_superseded",
            completed_records_retire_only_when_superseded,
        ),
        ("abandoned_inputs_are_removed", abandoned_inputs_are_removed),
        ("endings_outlive_queue_rows", endings_outlive_queue_rows),
        (
            "cancellation_keeps_inflight_ending_marker",
            cancellation_keeps_an_inflight_ending_from_replacing_its_marker,
        ),
        (
            "retirement_forgets_an_active_source",
            retirement_forgets_an_active_source,
        ),
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
    retirement_continues_after_corrupt_node(&database.create("pb_retirement"));
    println!("test retirement_continues_after_corrupt_node ... ok");
    through_controller(&database.create("pb_pass"), false, false, false);
    println!("test recorded_records_reach_kernel ... ok");
    through_controller(&database.create("pb_footprint"), true, false, false);
    println!("test outside_footprint_fails_round ... ok");
    through_controller(&database.create("pb_cancel"), false, true, false);
    println!("test cancelling_unbound_checks_starts_no_work ... ok");
    through_controller(&database.create("pb_recover"), false, false, true);
    println!("test controller_recovers_failed_publication ... ok");
    through_controller(&database.create("pb_cancel_sealed"), false, true, true);
    println!("test cancelling_never_published_seal_starts_no_work ... ok");
    database.stop();
    println!("26 PrismaBuild adapter scenarios passed; no nested submissions; no skips");
}

fn declared_wait_uses_bell() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let bell = Bell::new();
    let mut adapter = fixture_with_bell(root.path(), "pending", &bell.url);
    let key: tl3_prismabuild::ActionKey = "a".repeat(64).parse().unwrap();
    let wait = tl3_store::ExternalWait {
        upstream: "prismabuild".into(),
        key: key.to_string(),
    };
    let (sender, receiver) = mpsc::channel();
    let running = adapter.source().spawn(sender);
    adapter.watch_wait(&delivery, Some(&wait)).unwrap();
    bell.request().reply(0, serde_json::json!([]));
    let waiting = bell.request();
    assert!(
        waiting.line.contains(key.as_str()),
        "the declared key is in the bell's awaited set"
    );
    while receiver.try_recv().is_ok() {}
    fs::write(
        root.path().join(format!("queue/done/{key}.json")),
        serde_json::json!({
            "schema":"prismaquant.prismabuild.pool_outcome.v1", "action_key":key,
            "published_unix":100, "status":"executed", "detail":{"returncode":0}
        })
        .to_string(),
    )
    .unwrap();
    waiting.reply(1, serde_json::json!([{"seq":1, "dir":"done", "key":key}]));
    assert!(
        receiver.recv_timeout(Duration::from_secs(10)).is_ok(),
        "one ending wakes its declaring allowance"
    );
    assert!(receiver.try_recv().is_err(), "one event causes one look");
    adapter.watch_wait(&delivery, None).unwrap();
    drop(running);
}

fn source_skips_bad_waits() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut adapter = fixture(root.path(), "pending");
    let mut source = adapter.source();
    let wait = tl3_store::ExternalWait {
        upstream: "prismabuild".into(),
        key: "a".repeat(64),
    };
    adapter.watch_wait(&delivery, Some(&wait)).unwrap();
    let directory = root.path().join("records/0");
    fs::write(directory.join("broken.wait"), "broken JSON").unwrap();
    let valid = fs::read(record_path(root.path(), &delivery.key, "wait")).unwrap();
    let mut invalid: serde_json::Value = serde_json::from_slice(&valid).unwrap();
    invalid["wait"]["key"] = serde_json::json!("invalid-key");
    fs::write(directory.join("invalid-key.wait"), invalid.to_string()).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (tx, rx) = mpsc::channel();
    assert!(
        runtime
            .block_on(source.poll(&tx, Duration::from_secs(10)))
            .is_ok(),
        "rebuild skips bad wait subscriptions"
    );
    adapter.watch_wait(&delivery, None).unwrap();
    assert!(
        runtime
            .block_on(source.poll(&tx, Duration::from_secs(10)))
            .is_ok(),
        "registration skips bad wait subscriptions"
    );
    adapter.execute(&delivery).unwrap();
    assert!(
        runtime
            .block_on(source.poll(&tx, Duration::from_secs(10)))
            .is_ok()
    );
    let key = adapter.action_key(&delivery).unwrap().unwrap();
    finish(root.path(), key.as_str(), "done");
    assert_eq!(
        runtime
            .block_on(source.poll(&tx, Duration::from_secs(10)))
            .unwrap(),
        1,
        "a bad wait cannot stop an unrelated job completion"
    );
    assert_passing_input(rx.recv_timeout(Duration::from_secs(10)).unwrap());
}

fn live_waits_do_not_look_on_bell_reset() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut bell = Bell::new();
    let mut adapter = fixture_with_bell(root.path(), "pending", &bell.url);
    let key = "a".repeat(64);
    adapter
        .watch_wait(
            &delivery,
            Some(&tl3_store::ExternalWait {
                upstream: "prismabuild".into(),
                key: key.clone(),
            }),
        )
        .unwrap();
    let mut source = adapter.source();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let reset = serde_json::json!({"epoch":"0123456789abcdef0123456789abcdef", "seq":0, "reset":true, "events":[]});
    let mut poll = |response: serde_json::Value| {
        thread::scope(|scope| {
            let bell = &mut bell;
            scope.spawn(move || bell.request().response.send(response).unwrap());
            runtime
                .block_on(source.poll(&tx, Duration::from_secs(10)))
                .unwrap()
        })
    };
    for state in ["ready", "claimed"] {
        let path = root.path().join(format!("queue/{state}/{key}.json"));
        fs::write(&path, serde_json::json!({"schema":"prismaquant.prismabuild.pool_item.v1", "action_key":key, "published_unix":100}).to_string()).unwrap();
        for response in [reset.clone(), serde_json::json!({}), serde_json::json!({})] {
            assert_eq!(
                poll(response),
                0,
                "a live {state} record on reset or bell outage needs no look"
            );
            assert!(
                rx.try_recv().is_err(),
                "bell retries do not wake a live wait"
            );
        }
        fs::remove_file(path).unwrap();
    }
    assert_eq!(
        poll(reset.clone()),
        1,
        "a missing record on reset wakes its wait"
    );
    assert!(
        matches!(rx.try_recv().unwrap(), tl3_controller::cli::Input::Look { key } if key == delivery.key)
    );
    fs::write(
        root.path().join(format!("queue/done/{key}.json")),
        serde_json::json!({"schema":"prismaquant.prismabuild.pool_outcome.v1", "action_key":key, "published_unix":100, "status":"executed", "detail":{"returncode":0}}).to_string(),
    ).unwrap();
    assert_eq!(
        poll(reset.clone()),
        1,
        "an ended record on reset wakes its wait"
    );
    assert!(matches!(
        rx.try_recv().unwrap(),
        tl3_controller::cli::Input::Look { .. }
    ));
    fs::write(
        root.path().join(format!("queue/done/{key}.json")),
        "broken JSON",
    )
    .unwrap();
    assert_eq!(
        poll(reset),
        1,
        "an unreadable record on reset wakes its wait"
    );
    assert!(matches!(
        rx.try_recv().unwrap(),
        tl3_controller::cli::Input::Look { .. }
    ));
    assert!(rx.try_recv().is_err());
}

fn moving_wait_bell_is_quiet() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut bell = Bell::new();
    let mut adapter = fixture_with_bell(root.path(), "pending", &bell.url);
    let key = "a".repeat(64);
    let marker = root
        .path()
        .join(format!("queue/claimed/{key}.100.host.1.abc.tombstone"));
    fs::write(&marker, "{}").unwrap();
    adapter
        .watch_wait(
            &delivery,
            Some(&tl3_store::ExternalWait {
                upstream: "prismabuild".into(),
                key: key.clone(),
            }),
        )
        .unwrap();
    let mut source = adapter.source();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let reset = serde_json::json!({"epoch":"0123456789abcdef0123456789abcdef", "seq":0, "reset":true, "events":[]});
    for response in [
        reset,
        serde_json::json!({"epoch":"0123456789abcdef0123456789abcdef", "seq":1, "reset":false,
        "events":[{"seq":1,"dir":"claimed","key":key}]}),
    ] {
        let count = thread::scope(|scope| {
            let bell = &mut bell;
            scope.spawn(move || bell.request().response.send(response).unwrap());
            runtime
                .block_on(source.poll(&tx, Duration::from_secs(10)))
                .unwrap()
        });
        assert_eq!(count, 0, "finish window must produce no Look");
        assert!(rx.try_recv().is_err(), "moving record is no bell news");
    }
    assert!(
        record_path(root.path(), &delivery.key, "wait").is_file(),
        "move retains durable wait"
    );
    fs::write(
        root.path().join(format!("queue/done/{key}.json")),
        serde_json::json!({"schema":"prismaquant.prismabuild.pool_outcome.v1", "action_key":key,
            "published_unix":100,"status":"executed","detail":{"returncode":0}})
        .to_string(),
    )
    .unwrap();
    fs::remove_file(marker).unwrap();
    let count = thread::scope(|scope| {
        let bell = &mut bell;
        let key = &key;
        scope.spawn(move || {
            bell.request()
                .reply(2, serde_json::json!([{"seq":2,"dir":"done","key":key}]))
        });
        runtime
            .block_on(source.poll(&tx, Duration::from_secs(10)))
            .unwrap()
    });
    assert_eq!(count, 1, "retained wait receives its ending");
    assert!(
        matches!(rx.try_recv().unwrap(), tl3_controller::cli::Input::Look {key} if key == delivery.key)
    );
}

fn moving_job_reconcile_is_pending() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut adapter = fixture(root.path(), "pending");
    adapter.execute(&delivery).unwrap();
    let key = adapter.action_key(&delivery).unwrap().unwrap();
    fs::rename(
        root.path().join(format!("queue/ready/{key}.json")),
        root.path()
            .join(format!("queue/claimed/{key}.100.host.1.abc.tombstone")),
    )
    .unwrap();
    let result = adapter.reconcile(&delivery);
    assert!(
        matches!(result, Ok(Reconciliation::Pending)),
        "moving reconcile must stay Pending"
    );
}
