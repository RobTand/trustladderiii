#[path = "support/ownership.rs"]
mod startup_ownership;
// Single-event fixture receives expose no intermediate progress.
// #673 measured the full PrismaBuild binary at 220.814 s; 450 s > 2x.
const SIGNAL_WAIT: std::time::Duration = std::time::Duration::from_secs(450);

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
use tl3_kernel::{Command as Control, EffectKind, NodeId, Phase};
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
        "from value import value\ndef test_value(): assert value() == 2, 'old value'\n",
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
        scope_removals: None,
        steering: None,
        request_text: None,
        evidence: None,
        proposed: None,
        question_context: None,
        question: None,
        job_generation: Some(4),
        checks: Some(CheckRun {
            repository: repo,
            base_sha: tl3_worktrees::CommitId::new(base).unwrap(),

            reviewed_sha: None,
            reproducing_unavailable: None,
            selected_failures: Vec::new(),
            plan: vec![Check {
                name: "value".into(),
                kind: Kind::Demonstrating,
                tests: vec!["test_value.py::test_value".into()],
                forcing: None,
                proof: Some(
                    serde_json::from_value(
                        serde_json::json!({"subjects":[], "observer_support":[], "red_message":"old value"}),
                    )
                    .unwrap(),
                ),
                declared_symptom: None,
            }],
        }),
    }
}
fn fixture(root: &Path, mode: &str) -> Adapter {
    fixture_with_bell(root, mode, "http://127.0.0.1:9/")
}
fn fixture_with_bell(root: &Path, mode: &str, bell_url: &str) -> Adapter {
    let (client, settings) = fixture_settings(root, mode, bell_url);
    Adapter::new(client, settings, &mut crate::startup_ownership::ownership()).unwrap()
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
        Reconciliation::Found(completion) => *completion,
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
fn memory_oom_is_retained_in_check_evidence() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut adapter = fixture(root.path(), "memory_limit_oom");
    adapter.execute(&delivery).unwrap();
    let ended = completion(&mut adapter, &delivery);
    let encoded = serde_json::to_string(&ended).unwrap();
    assert!(
        encoded.contains("memory_limit_oom"),
        "retain the PB termination reason: {encoded}"
    );
    assert!(encoded.contains("2147483648"), "retain the scope peak");
    assert!(encoded.contains("137"), "retain the return code");
    assert!(
        !observed(ended).0,
        "OOM is infrastructure evidence, not a test report"
    );
}

fn harness_error_is_named_tl3_side() {
    // #326: the trusted harness rejected TL3's own check input inside the action.
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut adapter = fixture(root.path(), "harness_error");
    adapter.execute(&delivery).unwrap();
    let ended = completion(&mut adapter, &delivery);
    assert_eq!(ended.failure_class.as_deref(), Some("harness"));
    assert!(!observed(ended).0, "no check report");
    for mode in ["failed", "no_report", "infrastructure"] {
        let root = tempfile::tempdir().unwrap();
        let delivery = run(root.path(), false);
        let mut adapter = fixture(root.path(), mode);
        adapter.execute(&delivery).unwrap();
        assert_eq!(
            completion(&mut adapter, &delivery).failure_class,
            None,
            "{mode}"
        );
    }
}
/// #326: a restarted request's plan kept the replaced request's base B0, but its
/// worktree branch starts at B1, after main changed conftest.py. Returns the delivery,
/// its head and B1.
fn moved_base(root: &Path) -> (Delivery, String, String) {
    let mut delivery = run(root, false);
    let repo = root.join("restarted");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "-b", "main", "--template="]);
    fs::write(
        repo.join("pyproject.toml"),
        "[tool.tl3.harness]\ncommand = ['python', '-m', 'pytest']\ntest-files = ['test_*.py']\n",
    )
    .unwrap();
    fs::write(repo.join("conftest.py"), "# version 1\n").unwrap();
    fs::write(repo.join("value.py"), "def value(): return 1\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "pinned main"]);
    let pinned = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("conftest.py"), "# version 2\n").unwrap();
    git(&repo, &["commit", "--quiet", "-am", "main moves"]);
    let main = git(&repo, &["rev-parse", "HEAD"]);
    git(
        &repo,
        &["checkout", "--quiet", "-b", "prismabuild-42-tl3-1"],
    );
    git(
        &repo,
        &["update-ref", "refs/tl3/bases/prismabuild-42-tl3-1", &main],
    );
    fs::write(repo.join("value.py"), "def value(): return 2\n").unwrap();
    git(&repo, &["commit", "--quiet", "-am", "maker"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let run = delivery.checks.as_mut().unwrap();
    run.repository = repo;
    run.base_sha = tl3_worktrees::CommitId::new(&pinned).unwrap();
    delivery.head_sha = Some(head.clone());
    (delivery, head, main)
}

fn checks_use_the_base_the_head_descends_from() {
    let root = tempfile::tempdir().unwrap();
    let (delivery, head, main) = moved_base(root.path());
    let mut adapter = fixture(root.path(), "done");
    adapter.execute(&delivery).unwrap();
    let staged: Vec<_> = fs::read_dir(root.path().join("queue"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "request"))
        .collect();
    assert_eq!(staged.len(), 1);
    let request: serde_json::Value =
        serde_json::from_slice(&fs::read(&staged[0]).unwrap()).unwrap();
    assert_eq!(
        request["base"],
        main.as_str(),
        "stage the worktree's recorded base"
    );
    let ended = completion(&mut adapter, &delivery);
    assert!(
        ended.observations.iter().any(|observation| matches!(observation,
            Observation::CheckPlan { sha, run, .. } if *sha == head && run.base_sha.as_str() == main)),
        "record the corrected plan for the head"
    );
    assert!(
        ended
            .observations
            .iter()
            .any(|observation| matches!(observation,
        Observation::Checks { base_sha, .. } if *base_sha == main))
    );
}
/// #378: a completion whose staging moved the base carries CheckPlan and Checks. The source
/// must send them under distinct, stable keys, so the controller commits both, a replay
/// after a crash before `.done` is idempotent, and a stored pre-#350 `Checks` row still
/// deduplicates.
fn base_moved_completion_keys_each_observation(
    settings: &tl3_config::Store,
    legacy: &tl3_config::Store,
) {
    let root = tempfile::tempdir().unwrap();
    let (delivery, head, main) = moved_base(root.path());
    let mut adapter = fixture(root.path(), "pending");
    adapter.execute(&delivery).unwrap();
    let key = adapter.action_key(&delivery).unwrap().unwrap();
    finish(root.path(), key.as_str(), "done");
    let delivered = |mode: &str| {
        let (sender, receiver) = mpsc::channel();
        // The source stops when its adapter's channel closes; keep the adapter alive.
        let adapter = fixture(root.path(), mode);
        let running = adapter.source().spawn(sender);
        let inputs = [(); 2].map(|()| receiver.recv_timeout(SIGNAL_WAIT).unwrap());
        drop(running);
        inputs
    };
    let open = |settings: &tl3_config::Store| {
        Controller::open(
            tl3_store::Store::open(settings).unwrap(),
            adapters::simulation(&root.path().join("fakes")).unwrap(),
            Bootstrap {
                operations: None,
                repos: vec![repository()],
                rob: 1,
                slots: 1,
                pool: 1,
                fail_fast: false,
            },
        )
        .unwrap()
    };
    let crash_before_done = || {
        for entry in fs::read_dir(root.path().join("records/0")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "done") {
                fs::remove_file(path).unwrap();
            }
        }
    };

    let inputs = delivered("done");
    let mut controller = open(settings);
    for input in inputs.clone() {
        tl3_controller::ingress::apply(&mut controller, input)
            .expect("tl3d keeps running after a base-moved completion");
    }
    let keys: Vec<_> = inputs
        .iter()
        .map(|input| match input {
            tl3_controller::cli::Input::Observation { key, observation } => {
                (key.clone(), observation.clone())
            }
            _ => panic!("PrismaBuild sends observations"),
        })
        .collect();
    assert_eq!(keys[0].0, format!("pb:{key}:plan"));
    assert!(matches!(&keys[0].1, Observation::CheckPlan { sha, run, .. }
        if *sha == head && run.base_sha.as_str() == main));
    assert_eq!(
        keys[1].0,
        format!("pb:{key}"),
        "Checks keeps its pre-#350 key"
    );
    assert!(matches!(&keys[1].1, Observation::Checks { base_sha, .. } if *base_sha == main));

    crash_before_done();
    let replayed = delivered("done");
    assert_eq!(
        serde_json::to_value(&replayed).unwrap(),
        serde_json::to_value(&inputs).unwrap(),
        "a replay sends the same keys and payloads"
    );
    let before = controller.snapshot().unwrap();
    for input in replayed.clone() {
        tl3_controller::ingress::apply(&mut controller, input)
            .expect("a replay after a crash before .done is idempotent");
    }
    assert_eq!(controller.snapshot().unwrap(), before);
    drop(controller);

    // A pre-#350 release stored this action's Checks alone under `pb:{key}`.
    let mut controller = open(legacy);
    controller
        .observe(&format!("pb:{key}"), keys[1].1.clone())
        .unwrap();
    for input in replayed {
        tl3_controller::ingress::apply(&mut controller, input)
            .expect("an old-shape stored Checks row replays cleanly");
    }
}

/// #390: the harness refuses changes to the repository's protected paths, so the staged
/// request carries the same list planning uses: the defaults, the global list and the
/// repository's own additions.
fn staged_request_carries_the_repository_protected_paths() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut config = tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
    config.github.repos[0].protected_paths = vec!["release/**".into()];
    config.planning.protected_paths = vec!["GLOBAL.md".into()];
    let mut adapter = fixture(root.path(), "done")
        .with_repositories(&config.github.repos)
        .with_protected_paths(&config.planning.protected_paths);
    adapter.execute(&delivery).unwrap();
    let staged: Vec<_> = fs::read_dir(root.path().join("queue"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "request"))
        .collect();
    let request: serde_json::Value =
        serde_json::from_slice(&fs::read(&staged[0]).unwrap()).unwrap();
    let protected: BTreeSet<_> = request["protected_paths"]
        .as_array()
        .expect("the staged request names protected paths")
        .iter()
        .map(|path| path.as_str().unwrap())
        .collect();
    for expected in [".github/**", "**/conftest.py", "GLOBAL.md", "release/**"] {
        assert!(protected.contains(expected), "{expected} in {protected:?}");
    }
}

fn always_tests_are_staged_required_and_reported_in_every_round() {
    const ALWAYS: &str = "Repository always tests";
    for round in [0, 1] {
        for pre_existing in [false, true] {
            for omitted in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let mut delivery = run(root.path(), false);
                delivery.round = round;
                let mut config =
                    tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
                config.github.repos[0].always_tests = vec!["test_rules.py".into()];
                let mut adapter =
                    fixture(root.path(), "done").with_repositories(&config.github.repos);
                let planned = &delivery.checks.as_ref().unwrap().plan[0];
                let mut results = vec![
                    serde_json::json!({"name": planned.name, "kind": "demonstrating", "custody": "in-process",
                    "verdict": "pass", "tests": [{"test": planned.tests[0], "base": "assertion", "head": "passed", "pre_existing": false, "passed": true}]}),
                ];
                if !omitted {
                    results.push(serde_json::json!({"name": ALWAYS, "kind": "regression", "custody": "in-process",
                        "verdict": if pre_existing { "pass" } else { "fail" }, "tests": [{"test": "test_rules.py", "base": if pre_existing { "assertion" } else { "passed" },
                        "head": "assertion", "pre_existing": pre_existing, "passed": pre_existing, "message": "Tests must not sleep"}]}));
                }
                fs::write(
                    root.path().join("queue/recorded_results.json"),
                    serde_json::to_vec(&results).unwrap(),
                )
                .unwrap();
                adapter.execute(&delivery).unwrap();
                let staged = fs::read_dir(root.path().join("queue"))
                    .unwrap()
                    .map(|p| p.unwrap().path())
                    .find(|p| p.extension().is_some_and(|e| e == "request"))
                    .unwrap();
                let request: serde_json::Value =
                    serde_json::from_slice(&fs::read(staged).unwrap()).unwrap();
                assert_eq!(
                    request["always_tests"],
                    serde_json::json!(["test_rules.py"])
                );
                match &completion(&mut adapter, &delivery).observations[..] {
                    [
                        Observation::Checks {
                            report,
                            pass,
                            failed_tests,
                            ..
                        },
                    ] => {
                        assert_eq!(*report, !omitted);
                        assert_eq!(*pass, pre_existing && !omitted);
                        if !pre_existing && !omitted {
                            assert_eq!(failed_tests[0].test, "test_rules.py");
                            assert_eq!(failed_tests[0].message, "Tests must not sleep");
                        }
                    }
                    other => panic!("expected check feedback: {other:?}"),
                }
            }
        }
    }
}

fn stale_always_test_notes_survive_controller_evidence() {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut config = tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
    config.github.repos[0].always_tests = vec!["test_removed.py".into()];
    let mut adapter = fixture(root.path(), "done").with_repositories(&config.github.repos);
    let planned = &delivery.checks.as_ref().unwrap().plan[0];
    let note = "Ignored stale always_tests entry `test_removed.py`: missing on both the base and the head.";
    fs::write(root.path().join("queue/recorded_results.json"), serde_json::to_vec(&serde_json::json!([
        {"name": planned.name, "kind": "demonstrating", "custody": "in-process", "verdict": "pass", "tests": [{"test": planned.tests[0], "base": "assertion", "head": "passed", "pre_existing": false, "passed": true}]},
        {"name": "Repository always tests", "kind": "regression", "custody": "in-process", "verdict": "pass", "tests": [{"test": "test_removed.py", "base": "missing", "head": "missing", "pre_existing": false, "passed": true, "message": note}]}
    ])).unwrap()).unwrap();
    adapter.execute(&delivery).unwrap();
    match &completion(&mut adapter, &delivery).observations[..] {
        [
            Observation::Checks {
                report,
                pass,
                failed_tests,
                results,
                ..
            },
        ] => {
            assert!(*report && *pass);
            assert!(failed_tests.is_empty());
            assert_eq!(results[1]["tests"][0]["message"], note);
        }
        other => panic!("expected nonblocking evidence: {other:?}"),
    }
}

/// #359: a repository with `selected_tests` stages the limit, and the harness's one
/// trailing regression check of selected existing tests is accepted, failing the round
/// when it fails. Without the limit, the same trailing result is an invalid report.
fn selected_tests_are_staged_and_their_check_accepted() {
    const SELECTED: &str = "Existing tests that import the change";
    let ended = |selected_tests: Option<u32>| {
        let root = tempfile::tempdir().unwrap();
        let delivery = run(root.path(), false);
        let mut config =
            tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        config.github.repos[0].selected_tests = selected_tests;
        let mut adapter = fixture(root.path(), "done").with_repositories(&config.github.repos);
        let planned = &delivery.checks.as_ref().unwrap().plan[0];
        fs::write(
            root.path().join("queue/recorded_results.json"),
            serde_json::to_vec(&serde_json::json!([
                {"name": planned.name, "kind": "demonstrating", "custody": "in-process",
                 "verdict": "pass", "tests": [{"test": planned.tests[0], "base": "assertion",
                 "head": "passed", "pre_existing": false, "passed": true}]},
                {"name": SELECTED, "kind": "regression", "custody": "in-process",
                 "verdict": "fail", "tests": [{"test": "test_other.py::test_other", "base": "passed",
                 "head": "assertion", "pre_existing": false, "passed": false,
                 "message": "assert 2 == 1"}]},
            ]))
            .unwrap(),
        )
        .unwrap();
        adapter.execute(&delivery).unwrap();
        let staged: Vec<_> = fs::read_dir(root.path().join("queue"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|e| e == "request"))
            .collect();
        let request: serde_json::Value =
            serde_json::from_slice(&fs::read(&staged[0]).unwrap()).unwrap();
        (request, completion(&mut adapter, &delivery))
    };

    let (request, ended_with) = ended(Some(25));
    assert_eq!(request["selected_tests"], 25);
    match &ended_with.observations[..] {
        [
            Observation::Checks {
                report,
                pass,
                failed_tests,
                results,
                ..
            },
        ] => {
            assert!(
                *report && !*pass,
                "a failing selected check fails the round"
            );
            assert_eq!(results.len(), 2);
            assert_eq!(failed_tests[0].test, "test_other.py::test_other");
        }
        other => panic!("expected one check observation, got {other:?}"),
    }

    let (request, ended_without) = ended(None);
    assert!(request.get("selected_tests").is_none());
    assert!(
        matches!(
            &ended_without.observations[..],
            [Observation::Checks {
                report: false,
                pass: false,
                ..
            }]
        ),
        "an unrequested extra check is an invalid report"
    );
}

fn phase_evidence_survives_exact_head_receipts() {
    use serde_json::json;
    let phases = json!({"base":{"test":"test_value.py::test_value",
        "setup":{"outcome":"passed","exception":"","assertion":false},
        "call":{"outcome":"failed","exception":"builtins.AssertionError","assertion":true},
        "teardown":{"outcome":"failed","exception":"builtins.RuntimeError","assertion":false}},
        "head":{"test":"test_value.py::test_value",
        "setup":{"outcome":"passed","exception":"","assertion":false},
        "call":{"outcome":"passed","exception":"","assertion":false},
        "teardown":{"outcome":"passed","exception":"","assertion":false}}});
    for damage in [
        "none",
        "wrong_head",
        "digest",
        "receipt",
        "unknown",
        "oversized",
        "order",
        "assertion",
    ] {
        let root = tempfile::tempdir().unwrap();
        let delivery = run(root.path(), false);
        let mut evidence = phases.clone();
        match damage {
            "unknown" => evidence["base"]["call"]["locals"] = json!({"request":"untrusted"}),
            "oversized" => evidence["base"]["teardown"]["exception"] = "x".repeat(201).into(),
            "order" => {
                evidence["base"].as_object_mut().unwrap().remove("setup");
            }
            "assertion" => evidence["base"]["teardown"]["assertion"] = true.into(),
            _ => (),
        }
        let mut adapter = fixture(
            root.path(),
            if damage == "wrong_head" {
                "wrong_head"
            } else {
                "done"
            },
        );
        fs::write(root.path().join("queue/recorded_results.json"), serde_json::to_vec(&json!([{
            "name":"value", "kind":"demonstrating", "custody":"in-process", "verdict":"fail",
            "tests":[{"test":"test_value.py::test_value", "base":"error", "head":"passed",
                "pre_existing":false, "passed":false, "message":"Call assertion and teardown RuntimeError.", "phases":evidence}]
        }])).unwrap()).unwrap();
        adapter.execute(&delivery).unwrap();
        if matches!(damage, "digest" | "receipt") {
            let ending = fs::read_dir(root.path().join("queue"))
                .unwrap()
                .map(|e| e.unwrap().path())
                .find(|p| p.extension().is_some_and(|e| e == "ending"))
                .unwrap();
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&ending).unwrap()).unwrap();
            if damage == "digest" {
                value["endings"][0]["result"]["stdout"]["sha256"] = "b".repeat(64).into();
            } else {
                value["endings"][0]["result"]["receipt_sha256"] = serde_json::Value::Null;
            }
            fs::write(ending, serde_json::to_vec(&value).unwrap()).unwrap();
        }
        let ended = completion(&mut adapter, &delivery);
        match &ended.observations[0] {
            Observation::Checks {
                report,
                pass,
                results,
                ..
            } => {
                assert_eq!(*report, damage == "none", "{damage}");
                assert!(
                    !pass,
                    "a call assertion with teardown failure must remain invalid RED"
                );
                if *report {
                    assert_eq!(
                        results[0]["tests"][0]["phases"], phases,
                        "review must retain every phase"
                    );
                }
            }
            other => panic!("expected check evidence, got {other:?}"),
        }
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

/// #393: a forced demonstrating check stages its forcing for the harness, and its
/// result reaches review with the forcing and the forced head outcome. A forced
/// check's result without `head_forced` is an invalid report.
fn forced_check_is_staged_and_reaches_review() {
    let forcing = serde_json::json!({
        "symptom": "A real, unrelated process holds PID 4242.", "environment": {},
        "patches": [{"target": "sampler.process_in_scope", "raises": "ValueError"}]});
    let ended = |head_forced: bool| {
        let root = tempfile::tempdir().unwrap();
        let mut delivery = run(root.path(), false);
        let mut value = serde_json::to_value(delivery.checks.as_ref().unwrap()).unwrap();
        value["plan"][0]["forcing"] = forcing.clone();
        delivery.checks = Some(serde_json::from_value::<CheckRun>(value).unwrap());
        let planned = &delivery.checks.as_ref().unwrap().plan[0];
        let mut test = serde_json::json!({"test": planned.tests[0], "base": "assertion",
            "head": "passed", "pre_existing": false, "passed": true});
        if head_forced {
            test["head_forced"] = "passed".into();
        }
        let mut adapter = fixture(root.path(), "done");
        fs::write(
            root.path().join("queue/recorded_results.json"),
            serde_json::to_vec(&serde_json::json!([
                {"name": planned.name, "kind": "demonstrating", "custody": "in-process",
                 "verdict": "pass", "forcing": forcing, "tests": [test]}]))
            .unwrap(),
        )
        .unwrap();
        adapter.execute(&delivery).unwrap();
        let staged = fs::read_dir(root.path().join("queue"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|e| e == "request"))
            .unwrap();
        let request: serde_json::Value =
            serde_json::from_slice(&fs::read(staged).unwrap()).unwrap();
        (request, completion(&mut adapter, &delivery))
    };
    let (request, ended_with) = ended(true);
    assert_eq!(
        request["plan"][0]["forcing"], forcing,
        "the harness receives the forcing"
    );
    match &ended_with.observations[..] {
        [
            Observation::Checks {
                report,
                pass,
                results,
                ..
            },
        ] => {
            assert!(*report && *pass);
            assert_eq!(results[0]["forcing"], forcing, "review sees the forcing");
            assert_eq!(results[0]["tests"][0]["head_forced"], "passed");
        }
        other => panic!("expected one check observation, got {other:?}"),
    }
    let (_, ended_without) = ended(false);
    assert!(
        matches!(
            &ended_without.observations[..],
            [Observation::Checks { report: false, .. }]
        ),
        "a forced check needs its forced head outcome"
    );
}

fn declared_symptom_identity_is_staged_and_reaches_review() {
    declared_symptom_identity(false);
}

fn review662_explicit_subject_symptom_keeps_receipt_bindings() {
    declared_symptom_identity(true);
}

fn declared_symptom_identity(explicit_subject: bool) {
    use sha2::{Digest, Sha256};
    for tamper in [
        "none",
        "base",
        "head",
        "forcing",
        "declaration",
        "receipt",
        "zero",
        "head_symptom",
        "missing",
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut delivery = run(root.path(), false);
        let mut value = serde_json::to_value(delivery.checks.as_ref().unwrap()).unwrap();
        let declaration = serde_json::json!({"node":value["plan"][0]["tests"][0],
            "exception":"builtins.TimeoutError", "phase":"call", "repeat":1, "parallel":0});
        value["plan"][0]["declared_symptom"] = declaration.clone();
        let proof = serde_json::json!({"subjects":["test_value.py::_git"],
            "observer_support":[], "red_message":"not an assertion"});
        if explicit_subject {
            value["plan"][0]["proof"] = proof.clone();
        }
        delivery.checks = Some(serde_json::from_value::<CheckRun>(value).unwrap());
        let planned = &delivery.checks.as_ref().unwrap().plan[0];
        let records = serde_json::json!([{"event":"start", "schema":1}]);
        let hash = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&records).unwrap())
        );
        let mut evidence = serde_json::json!({"base":delivery.checks.as_ref().unwrap().base_sha,
            "head":delivery.head_sha,"forcing":null,"declaration":declaration,
            "runs":[{"side":"base","run":1,"outcome":"symptom","sha256":hash,"records":records},
                    {"side":"head","run":1,"outcome":"passed","sha256":hash,"records":records}]});
        match tamper {
            "base" | "head" => evidence[tamper] = "0".repeat(40).into(),
            "forcing" => evidence["forcing"] = serde_json::json!({"environment":{"OTHER":"1"}}),
            "declaration" => evidence["declaration"]["repeat"] = 2.into(),
            "receipt" => evidence["runs"][0]["sha256"] = "0".repeat(64).into(),
            "zero" => evidence["runs"][0]["outcome"] = "passed".into(),
            "head_symptom" => evidence["runs"][1]["outcome"] = "symptom".into(),
            _ => (),
        }
        let mut result = serde_json::json!({"name":planned.name,"kind":"demonstrating","custody":"in-process",
            "verdict":"pass", "declared_symptom":declaration,"symptom_evidence":evidence,
            "tests":[{"test":planned.tests[0],"base":"error","head":"passed","pre_existing":false,"passed":true}]});
        if explicit_subject {
            let mut observers = serde_json::Map::new();
            observers.insert(planned.tests[0].clone(), "c".repeat(64).into());
            result["proof"] = serde_json::json!({"rule":"subject-observer-v1", "declaration":proof,
                "mode":"pytest-outcome", "subjects":{"test_value.py::_git":{"base":"a".repeat(64),"head":"b".repeat(64)}},
                "source_base_tree_sha256":"f".repeat(64), "base_tree_sha256":"d".repeat(64), "head_tree_sha256":"e".repeat(64),
                "observer_sha256":observers, "support_sha256":{}});
        }
        if tamper == "missing" {
            result.as_object_mut().unwrap().remove("symptom_evidence");
        }
        let mut adapter = fixture(root.path(), "done");
        fs::write(
            root.path().join("queue/recorded_results.json"),
            serde_json::to_vec(&serde_json::json!([result])).unwrap(),
        )
        .unwrap();
        adapter.execute(&delivery).unwrap();
        let ended = completion(&mut adapter, &delivery);
        match &ended.observations[..] {
            [
                Observation::Checks {
                    report,
                    pass,
                    results,
                    ..
                },
            ] => {
                assert_eq!(*report && *pass, tamper == "none", "{tamper}");
                if tamper == "none" {
                    assert_eq!(results[0]["symptom_evidence"], evidence);
                }
            }
            other => panic!("expected check observation, got {other:?}"),
        }
    }
}

fn subject_observer_identity_is_staged_and_reaches_review() {
    let declaration = serde_json::json!({"subjects":["test_value.py::_execute"],
        "observer_support":[], "red_message":"missing diagnostic"});
    for tamper in [
        "none",
        "declaration",
        "old_identity",
        "observer",
        "base_passed",
        "missing",
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut delivery = run(root.path(), false);
        let mut value = serde_json::to_value(delivery.checks.as_ref().unwrap()).unwrap();
        value["plan"][0]["proof"] = declaration.clone();
        delivery.checks = Some(serde_json::from_value::<CheckRun>(value).unwrap());
        let planned = &delivery.checks.as_ref().unwrap().plan[0];
        let mut observers = serde_json::Map::new();
        observers.insert(planned.tests[0].clone(), "c".repeat(64).into());
        let mut identity = serde_json::json!({"rule":"subject-observer-v1", "declaration":declaration,
            "mode":"head-observer", "subjects":{"test_value.py::_execute":{"base":"a".repeat(64),"head":"b".repeat(64)}},
            "source_base_tree_sha256":"f".repeat(64), "base_tree_sha256":"d".repeat(64), "head_tree_sha256":"e".repeat(64),
            "observer_sha256":observers, "support_sha256":{}});
        if tamper == "declaration" {
            identity["declaration"]["red_message"] = "different".into();
        }
        if tamper == "old_identity" {
            identity["subjects"]["test_value.py::_execute"]["base"] = "b".repeat(64).into();
        }
        if tamper == "observer" {
            identity["observer_sha256"] = serde_json::json!({});
        }
        let mut result = serde_json::json!({"name":planned.name,"kind":"demonstrating","custody":"in-process",
            "verdict":"pass","proof":identity,"tests":[{"test":planned.tests[0],
            "base":if tamper == "base_passed" {"passed"} else {"assertion"},"head":"passed","pre_existing":false,"passed":true}]});
        if tamper == "missing" {
            result.as_object_mut().unwrap().remove("proof");
        }
        let mut adapter = fixture(root.path(), "done");
        fs::write(
            root.path().join("queue/recorded_results.json"),
            serde_json::to_vec(&serde_json::json!([result])).unwrap(),
        )
        .unwrap();
        adapter.execute(&delivery).unwrap();
        let staged = fs::read_dir(root.path().join("queue"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "request"))
            .unwrap();
        let request: serde_json::Value =
            serde_json::from_slice(&fs::read(staged).unwrap()).unwrap();
        assert_eq!(request["plan"][0]["proof"], declaration);
        let ended = completion(&mut adapter, &delivery);
        match &ended.observations[..] {
            [
                Observation::Checks {
                    report,
                    pass,
                    results,
                    ..
                },
            ] => {
                assert_eq!(*report && *pass, tamper == "none", "{tamper}");
                if tamper == "none" {
                    assert_eq!(results[0]["proof"], identity);
                    assert_eq!(results[0]["tests"][0]["base"], "assertion");
                    assert_eq!(results[0]["tests"][0]["head"], "passed");
                }
            }
            other => panic!("expected check observation, got {other:?}"),
        }
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
        let deadline = std::time::Instant::now() + SIGNAL_WAIT;
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
        assert_passing_input(receiver.recv_timeout(SIGNAL_WAIT).unwrap());
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
    let observation = receiver.recv_timeout(SIGNAL_WAIT).unwrap();
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
        receiver.recv_timeout(SIGNAL_WAIT).unwrap(),
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
            .map(|_| receiver.recv_timeout(SIGNAL_WAIT).unwrap())
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
        let mut adapter =
            Adapter::new(client, settings, &mut crate::startup_ownership::ownership()).unwrap();
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
            assert_passing_input(receiver.recv_timeout(SIGNAL_WAIT).unwrap());
            drop(running);
        } else {
            fs::remove_file(root.path().join("queue/garbled_ending")).unwrap();
            waiting.reply(1, serde_json::json!([{"seq":1,"dir":"done","key":key}]));
            assert_passing_input(receiver.recv_timeout(SIGNAL_WAIT).unwrap());
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
    assert_passing_input(receiver.recv_timeout(SIGNAL_WAIT).unwrap());
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
    assert_passing_input(receiver.recv_timeout(SIGNAL_WAIT).unwrap());
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
        assert_passing_input(receiver.recv_timeout(SIGNAL_WAIT).unwrap());
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
    assert_passing_input(receiver.recv_timeout(SIGNAL_WAIT).unwrap());
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
    assert_passing_input(receiver.recv_timeout(SIGNAL_WAIT).unwrap());
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
    let deadline = std::time::Instant::now() + SIGNAL_WAIT;
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
    assert_passing_input(receiver.recv_timeout(SIGNAL_WAIT).unwrap());
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

#[derive(Clone)]
struct FakeClock(std::rc::Rc<std::cell::Cell<u64>>);
impl tl3_controller::clock::Clock for FakeClock {
    fn now(&self) -> std::time::SystemTime {
        std::time::UNIX_EPOCH + Duration::from_millis(self.0.get())
    }
}

/// #369: a checks job its own record shows queued is alive, however long it
/// stays quiet. PrismaBuild ends a quiet job itself. An unreadable record is
/// still a failed read, at the allowances' pace.
fn quiet_alive_job_is_not_a_failed_read(settings: &tl3_config::Store, unreadable: bool) {
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let adapter = fixture(root.path(), "pending");
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
    let clock = FakeClock(std::rc::Rc::new(std::cell::Cell::new(1_000_000)));
    let mut controller = Controller::open_with_clock(
        tl3_store::Store::open(settings).unwrap(),
        registry,
        Bootstrap {
            operations: None,
            repos: vec![repository()],
            rob: 1,
            slots: 1,
            pool: 1,
            fail_fast: false,
        },
        clock.clone(),
    )
    .unwrap();
    let mut allowances = tl3_config::Allowances::default();
    allowances.prismabuild.silence_secs = 1;
    allowances.prismabuild.no_progress_secs = 3;
    controller.set_allowances(allowances);
    controller
        .intake(LabelledIssue {
            delivery: "pb-quiet".into(),
            owner: "RobTand".into(),
            repo: "tl3-sandbox".into(),
            installation: 1,
            number: 42,
            title: "PrismaBuild fixture".into(),
            actor: 1,
            label: "tl3".into(),
            labels: Vec::new(),
        })
        .unwrap();
    while controller.drive_once().unwrap() {}
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
    for _ in 0..100 {
        if !controller.drive_once().unwrap() {
            break;
        }
    }
    let ready: Vec<_> = fs::read_dir(root.path().join("queue/ready"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(ready.len(), 1, "the checks job is queued and quiet");
    if unreadable {
        fs::write(&ready[0], "not a record").unwrap();
    }
    let prismabuild_incidents = |controller: &mut Controller| {
        controller
            .snapshot()
            .unwrap()
            .rows
            .into_iter()
            .filter(|r| r.table == "incidents" && r.value["signature"]["upstream"] == "PrismaBuild")
            .map(|r| r.value)
            .collect::<Vec<_>>()
    };
    // Well past the silence and no-progress allowances, with no test completing.
    for look in 1..=6 {
        clock.0.set(clock.0.get() + 2_000);
        controller.look_due().unwrap();
        let incidents = prismabuild_incidents(&mut controller);
        if !unreadable {
            assert!(
                incidents.is_empty(),
                "look {look}: a quiet job its record shows alive is not a failed read: {incidents:?}"
            );
        } else if look < 2 {
            assert!(
                incidents.is_empty(),
                "look {look}: one failure is not an incident"
            );
        }
    }
    if unreadable {
        let incidents = prismabuild_incidents(&mut controller);
        assert_eq!(incidents.len(), 1, "an unreadable record still raises one");
        assert_eq!(incidents[0]["open"], true);
    }
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Checking);
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
            fail_fast: false,
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
            labels: Vec::new(),
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
                Observation::Steering {
                    node: NodeId(0),
                    command: Control::Stop,
                },
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
                    Observation::Steering {
                        node: NodeId(0),
                        command: Control::Stop,
                    },
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
            fail_fast: false,
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
                labels: Vec::new(),
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
        println!("test router_retire_delivery ... ok");
        if std::env::args().len() > 1 {
            return;
        }
    }

    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--failure-phases") {
        phase_evidence_survives_exact_head_receipts();
        println!("test phase_evidence_survives_exact_head_receipts ... ok");
        println!("1 prismabuild scenarios passed");
        return;
    }
    if args.get(1).is_some_and(|arg| arg == "--repository-demand") {
        repository_test_demand_is_sealed_with_default_and_identity_fallback();
        return;
    }
    if args.get(1).is_some_and(|arg| arg == "--always-tests") {
        always_tests_are_staged_required_and_reported_in_every_round();
        stale_always_test_notes_survive_controller_evidence();
        return;
    }
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
    if args.get(1).is_some_and(|arg| arg == "--review-budgets") {
        let mut failures = 0;
        for test in [
            oversized_plan_is_refused_before_submission as fn(),
            result_budget_rejects_repeated_checks,
        ] {
            failures += usize::from(std::panic::catch_unwind(test).is_err());
        }
        let mut database = support::Database::start();
        let settings = database.create("pb_plan_budget");
        failures += usize::from(
            std::panic::catch_unwind(|| oversized_planner_reply_reaches_plan_feedback(&settings))
                .is_err(),
        );
        database.stop();
        assert_eq!(failures, 0);
        return;
    }
    if args.get(1).is_some_and(|arg| arg == "--report-limit") {
        oversized_plan_is_refused_before_submission();
        return;
    }
    if args.get(1).is_some_and(|arg| arg == "--repository-image") {
        repository_image_is_sealed_and_staged_with_global_fallback();
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
    if args.get(1).is_some_and(|arg| arg == "--quiet-looks") {
        let mut database = support::Database::start();
        quiet_alive_job_is_not_a_failed_read(&database.create("pb_quiet_alive"), false);
        quiet_alive_job_is_not_a_failed_read(&database.create("pb_unreadable"), true);
        database.stop();
        return;
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
            "oversized_plan_is_refused_before_submission",
            oversized_plan_is_refused_before_submission as fn(),
        ),
        (
            "repository_test_demand_is_sealed_with_default_and_identity_fallback",
            repository_test_demand_is_sealed_with_default_and_identity_fallback as fn(),
        ),
        (
            "repository_image_is_sealed_and_staged_with_global_fallback",
            repository_image_is_sealed_and_staged_with_global_fallback as fn(),
        ),
        (
            "moving_wait_bell_is_quiet",
            moving_wait_bell_is_quiet as fn(),
        ),
        (
            "moving_job_reconcile_is_pending",
            moving_job_reconcile_is_pending,
        ),
        ("job_look_states", job_look_states as fn()),
        (
            "reproducing_check_plan_and_verdict",
            reproducing_check_plan_and_verdict,
        ),
        (
            "forced_check_is_staged_and_reaches_review",
            forced_check_is_staged_and_reaches_review,
        ),
        (
            "phase_evidence_survives_exact_head_receipts",
            phase_evidence_survives_exact_head_receipts as fn(),
        ),
        (
            "declared_symptom_identity_is_staged_and_reaches_review",
            declared_symptom_identity_is_staged_and_reaches_review as fn(),
        ),
        (
            "review662_explicit_subject_symptom_keeps_receipt_bindings",
            review662_explicit_subject_symptom_keeps_receipt_bindings as fn(),
        ),
        (
            "subject_observer_identity_is_staged_and_reaches_review",
            subject_observer_identity_is_staged_and_reaches_review as fn(),
        ),
        ("source_skips_bad_waits", source_skips_bad_waits as fn()),
        (
            "live_waits_do_not_look_on_bell_reset",
            live_waits_do_not_look_on_bell_reset,
        ),
        ("declared_wait_uses_bell", declared_wait_uses_bell as fn()),
        (
            "memory_oom_is_retained_in_check_evidence",
            memory_oom_is_retained_in_check_evidence as fn(),
        ),
        ("recorded_endings", recorded_endings as fn()),
        (
            "retirement_refuses_active_or_unreadable_jobs",
            retirement_refuses_active_or_unreadable_jobs,
        ),
        ("bounded_failure_details", bounded_failure_details),
        (
            "stale_always_test_notes_survive_controller_evidence",
            stale_always_test_notes_survive_controller_evidence,
        ),
        (
            "always_tests_are_staged_required_and_reported_in_every_round",
            always_tests_are_staged_required_and_reported_in_every_round,
        ),
        (
            "selected_tests_are_staged_and_their_check_accepted",
            selected_tests_are_staged_and_their_check_accepted,
        ),
        (
            "staged_request_carries_the_repository_protected_paths",
            staged_request_carries_the_repository_protected_paths,
        ),
        (
            "harness_error_is_named_tl3_side",
            harness_error_is_named_tl3_side,
        ),
        (
            "checks_use_the_base_the_head_descends_from",
            checks_use_the_base_the_head_descends_from,
        ),
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
    oversized_planner_reply_reaches_plan_feedback(&database.create("pb_plan_budget"));
    println!("test oversized_planner_reply_reaches_plan_feedback ... ok");
    result_budget_rejects_repeated_checks();
    println!("test result_budget_rejects_repeated_checks ... ok");
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
    base_moved_completion_keys_each_observation(
        &database.create("pb_observation_keys"),
        &database.create("pb_observation_keys_legacy"),
    );
    println!("test base_moved_completion_keys_each_observation ... ok");
    quiet_alive_job_is_not_a_failed_read(&database.create("pb_quiet_alive"), false);
    println!("test quiet_alive_job_is_not_a_failed_read ... ok");
    quiet_alive_job_is_not_a_failed_read(&database.create("pb_unreadable"), true);
    println!("test unreadable_job_record_is_a_failed_read ... ok");
    database.stop();
    println!("53 PrismaBuild adapter scenarios passed; no nested submissions; no skips");
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
        receiver.recv_timeout(SIGNAL_WAIT).is_ok(),
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
    assert_passing_input(rx.recv_timeout(SIGNAL_WAIT).unwrap());
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

/// #369, #410 review: the look reads the job's own record. Queued, claimed
/// and moving are alive; an ended job, lease lost included, is found; a job
/// never submitted is absent.
fn job_look_states() {
    use tl3_controller::executor::Look;
    let root = tempfile::tempdir().unwrap();
    let delivery = run(root.path(), false);
    let mut adapter = fixture(root.path(), "pending");
    assert!(matches!(adapter.look(&delivery).unwrap(), Look::Absent));
    adapter.execute(&delivery).unwrap();
    let key = adapter.action_key(&delivery).unwrap().unwrap();
    let queue = root.path().join("queue");
    let alive = |adapter: &mut Adapter, state: &str| {
        let look = adapter.look(&delivery).unwrap();
        assert!(
            matches!(&look, Look::Alive { progress } if *progress == format!("prismabuild:{state}")),
            "{state}"
        );
    };
    alive(&mut adapter, "queued");
    fs::rename(
        queue.join(format!("ready/{key}.json")),
        queue.join(format!("claimed/{key}.json")),
    )
    .unwrap();
    alive(&mut adapter, "claimed");
    fs::rename(
        queue.join(format!("claimed/{key}.json")),
        queue.join(format!("claimed/{key}.100.host.1.abc.tombstone")),
    )
    .unwrap();
    alive(&mut adapter, "moving");
    fs::remove_file(queue.join(format!("claimed/{key}.100.host.1.abc.tombstone"))).unwrap();
    finish(root.path(), key.as_str(), "failed");
    let record = queue.join(format!("failed/{key}.json"));
    let lost = fs::read_to_string(&record).unwrap().replace(
        "\"termination_reason\": \"failed\"",
        "\"termination_reason\": \"lease_lost\"",
    );
    assert!(lost.contains("lease_lost"));
    fs::write(&record, lost).unwrap();
    assert!(
        matches!(adapter.look(&delivery).unwrap(), Look::Found(_)),
        "a job whose lease was lost has ended"
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

fn repository_test_demand_is_sealed_with_default_and_identity_fallback() {
    for (owner, name, installation, configured, cpus, mem_gb) in [
        ("robtand", "TL3-SANDBOX", 1, true, 4, 12),
        ("RobTand", "tl3-sandbox", 1, false, 1, 2),
        ("RobTand", "other", 1, true, 1, 2),
        ("Other", "tl3-sandbox", 1, true, 1, 2),
        ("RobTand", "tl3-sandbox", 2, true, 1, 2),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut delivery = run(root.path(), false);
        delivery.repository = Repository {
            owner: owner.into(),
            name: name.into(),
            installation,
        };
        let text = include_str!("../../config/example.toml");
        let text = if configured {
            text.replace(
                "installation_id = 1",
                "installation_id = 1\ntest_demand = { cpus = 4, mem_gb = 12 }",
            )
        } else {
            text.to_owned()
        };
        let config = tl3_config::Config::parse(&text).unwrap();
        let mut adapter = fixture(root.path(), "pending").with_repositories(&config.github.repos);
        adapter.execute(&delivery).unwrap();
        let calls: Vec<Vec<String>> = fs::read_to_string(root.path().join("queue/calls.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let seal = calls
            .iter()
            .find(|args| args.iter().any(|arg| arg == "--seal-only"))
            .unwrap();
        assert!(
            seal.windows(2)
                .any(|pair| pair == ["--cpus", &cpus.to_string()]),
            "expected {cpus} CPUs in {seal:?}"
        );
        assert!(
            seal.windows(2)
                .any(|pair| pair == ["--demand", &format!("mem_gb={mem_gb},gpu=0")]),
            "expected {mem_gb} GiB in {seal:?}"
        );
        for name in ["OMP_NUM_THREADS", "MKL_NUM_THREADS", "OPENBLAS_NUM_THREADS"] {
            assert!(
                seal.windows(2)
                    .any(|pair| pair == ["--env", &format!("{name}={cpus}")])
            );
        }
    }
}

fn repository_image_is_sealed_and_staged_with_global_fallback() {
    for (owner, name, installation, expected) in [
        ("robtand", "TL3-SANDBOX", 1, "b"),
        ("RobTand", "other", 1, "a"),
        ("Other", "tl3-sandbox", 1, "a"),
        ("RobTand", "tl3-sandbox", 2, "a"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut delivery = run(root.path(), false);
        delivery.repository = Repository {
            owner: owner.into(),
            name: name.into(),
            installation,
        };
        let config: tl3_config::Repository = serde_json::from_value(serde_json::json!({
            "owner":"RobTand", "name":"tl3-sandbox", "installation_id":1,
            "container_image":format!("sha256:{}", "b".repeat(64))
        }))
        .unwrap();
        let mut adapter = fixture(root.path(), "pending").with_repositories(&[config]);
        adapter.execute(&delivery).unwrap();
        let key = adapter.action_key(&delivery).unwrap().unwrap();
        let request: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join(format!("queue/{key}.request"))).unwrap(),
        )
        .unwrap();
        let image = format!("sha256:{}", expected.repeat(64));
        assert_eq!(request["image"], image, "staged harness image");
        let calls: Vec<Vec<String>> = fs::read_to_string(root.path().join("queue/calls.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let seal = calls
            .iter()
            .find(|args| args.iter().any(|arg| arg == "--seal-only"))
            .unwrap();
        assert!(
            seal.windows(2).any(|pair| pair[0] == "--env"
                && pair[1].starts_with("PRISMABUILD_EPHEMERAL_SCRATCH_DECLARATIONS=")),
            "Checking must seal the registered disk scratch lifetime"
        );
        let index = seal
            .iter()
            .position(|arg| arg == "--container-image")
            .unwrap();
        assert_eq!(
            seal[index + 1],
            image,
            "PB placement must use the staged image"
        );
    }
}

fn oversized_plan_is_refused_before_submission() {
    let root = tempfile::tempdir().unwrap();
    let mut delivery = run(root.path(), false);
    delivery.checks.as_mut().unwrap().plan[0].tests = vec![format!(
        "test_value.py::test_{}",
        "x".repeat(4 * 1024 * 1024)
    )];
    let mut adapter = fixture(root.path(), "pending");
    let result = adapter.execute(&delivery);
    assert!(
        result.is_ok(),
        "oversized plan must complete as a plan failure, not hold a setup error"
    );
    let completion = result.unwrap();
    assert!(matches!(&completion.observations[0], Observation::Checks {
        report: true, pass: false, failed_tests, ..
    } if failed_tests[0].message.contains("4194304") && failed_tests[0].message.contains("reduce")));
    assert!(adapter.action_key(&delivery).unwrap().is_none());
    let calls = fs::read_to_string(root.path().join("queue/calls.jsonl")).unwrap_or_default();
    assert!(!calls.contains("--seal-only") && !calls.contains("--publish"));
}

fn result_budget_rejects_repeated_checks() {
    let root = tempfile::tempdir().unwrap();
    let mut delivery = run(root.path(), false);
    delivery.checks.as_mut().unwrap().plan = (0..40000)
        .map(|i| Check {
            name: format!("check-{i}"),
            kind: Kind::Regression,
            tests: vec!["test_value.py::test_value".into()],
            forcing: None,
            proof: None,
            declared_symptom: None,
        })
        .collect();
    let mut adapter = fixture(root.path(), "pending");
    let completion = adapter.execute(&delivery).unwrap();
    assert!(
        matches!(completion.observations.first(), Some(Observation::Checks {
        report: true, pass: false, failed_tests, ..
    }) if failed_tests[0].message.contains("8388608") && failed_tests[0].message.contains("reduce")),
        "result budget must complete a plan failure before publication"
    );
    assert!(adapter.action_key(&delivery).unwrap().is_none());
}

fn oversized_planner_reply_reaches_plan_feedback(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let fakes = root.path().join("fakes");
    let oversized_replies = std::rc::Rc::new(std::cell::Cell::new(0));
    let replies = oversized_replies.clone();
    let mut registry = adapters::simulation(&fakes).unwrap();
    registry.register(Upstream::Workers, support::workers::Workers::new(
        None, adapters::workers::Fake::new(&fakes), vec![], move |_, completion: &mut Completion| {
            for observation in &mut completion.observations {
                if let Observation::Plan {node, generation, ..} = observation {
                    let forecast = serde_json::json!({"tokenizer":"o200k_base", "brief_tokens":1, "file_tokens":{"value.py":0}, "overhead_tokens":0,
                        "tokens":1, "fit_tokens":100, "context_tokens":150, "fits":true});
                    let reply = if replies.get() == 0 {
                        replies.set(replies.get() + 1);
                        serde_json::json!({
                            "kind":"leaf", "files":["value.py"], "checks":[{
                                "kind":"regression", "node_ids":[format!("test_value.py::test_{}", "x".repeat(4 * 1024 * 1024))], "goal_ids":[0]}],
                            "no_design_questions":true, "lands_on_its_own":true, "violations":[],
                            "forecast":forecast
                        })
                    } else {
                        serde_json::json!({"kind":"cant_meet", "files":["value.py"],
                            "report":"No smaller selection is available.", "violations":[], "forecast":forecast})
                    };
                    *observation = Observation::WorkerReply {node: *node, generation: *generation, role: "planner".into(), reply, plan_check: None, model: None};
                }
            }
        }));
    let mut controller = Controller::open(
        tl3_store::Store::open(settings).unwrap(),
        registry,
        Bootstrap {
            operations: None,
            repos: vec![repository()],
            rob: 1,
            slots: 1,
            pool: 1,
            fail_fast: false,
        },
    )
    .unwrap();
    controller
        .intake(LabelledIssue {
            delivery: "oversized-planner".into(),
            owner: "RobTand".into(),
            repo: "tl3-sandbox".into(),
            installation: 1,
            number: 42,
            title: "Plan budget".into(),
            actor: 1,
            label: "tl3".into(),
            labels: Vec::new(),
        })
        .unwrap();
    for _ in 0..100 {
        if !controller.drive_once().unwrap() {
            break;
        }
    }
    let world = controller.world().unwrap();
    assert_eq!(
        world.nodes[0].question.map(|q| q.kind),
        Some(tl3_kernel::QKind::CantMeet),
        "oversized plans must follow the normal planner retry and feedback path"
    );
    assert_eq!(world.nodes[0].run_incidents, 0);
    let mut store = tl3_store::Store::open(settings).unwrap();
    let findings = store.worker_document("0", "findings").unwrap().unwrap();
    assert!(
        findings
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("4194304")
                && v.as_str().unwrap().contains("reduce"))
    );
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .pending_effects
            .iter()
            .any(|e| e.kind == "PrismaBuild")
    );
    assert_eq!(
        oversized_replies.get(),
        1,
        "one oversized reply must prove refusal; the planner retry can decline with a small reply"
    );
}
