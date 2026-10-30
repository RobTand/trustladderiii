use super::*;
use tl3_controller::executor::{Completion, Registry, Upstream};
mod real;

pub(super) fn trial(settings: &tl3_config::Store, root: &Path) {
    real::Flow::start(settings, root).write_trial(root);
}

fn git(root: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().into()
}

fn repository_fixture(
    settings: &tl3_config::Store,
    root: &Path,
    mode: &str,
) -> (tl3_config::Config, PathBuf, PathBuf, String) {
    let mut config = super::config(settings, root, "normal");
    config.worktrees_root = root.join("worktrees");
    config.workers.command = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/maker/model.py")
            .to_str()
            .unwrap()
            .into(),
        mode.into(),
    ];
    let repository = root.join("repo");
    fs::create_dir(&repository).unwrap();
    git(
        &repository,
        &["init", "--quiet", "--template=", "-b", "main"],
    );
    fs::write(
        repository.join("greeting.py"),
        "def greet(): return 'goodbye'\n",
    )
    .unwrap();
    fs::write(repository.join("helper.py"), "# shared greeting\n").unwrap();
    fs::write(
        repository.join("test_greeting.py"),
        "def test_hello():\n    from greeting import greet\n    assert greet() == 'hello'\n",
    )
    .unwrap();
    fs::write(repository.join("pyproject.toml"), "[tool.tl3.harness]\ncommand = [\"python\", \"-m\", \"pytest\"]\ntest-files = [\"test_*.py\"]\n").unwrap();
    git(&repository, &["add", "."]);
    git(&repository, &["commit", "--quiet", "-m", "Base"]);
    let base = git(&repository, &["rev-parse", "HEAD"]);
    let worktree = config
        .worktrees_root
        .join("726f6274616e64/746c332d73616e64626f78/issue-42-node-0");
    Store::open(settings).unwrap().put_worker_materials("0", &json!({"maker": {
        "brief": "Return hello.", "goals": [{"id": "greeting", "description": "Return hello.", "verification": "test"}],
        "plan": {"files": ["not-authoritative.py"], "checks": [{"name": "hello", "kind": "demonstrating", "tests": ["test_greeting.py::test_hello"], "goal_ids": ["greeting"]}]},
        "attempt": 3, "round": 1
    }})).unwrap();
    (config, repository, worktree, base)
}

fn fixture(
    settings: &tl3_config::Store,
    root: &Path,
    mode: &str,
) -> (tl3_config::Config, PathBuf, String) {
    let (config, repository, worktree, base) = repository_fixture(settings, root, mode);
    fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    git(
        &repository,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "maker-fixture",
            worktree.to_str().unwrap(),
        ],
    );
    (config, worktree, base)
}

struct Workers {
    real: Supervisor,
    fake: adapters::workers::Fake,
}
impl Workers {
    fn plan(completion: &mut Completion) {
        for observation in &mut completion.observations {
            if let Observation::Plan { paths, .. } = observation {
                *paths = vec!["greeting.py".into(), "test_greeting.py".into()];
            }
        }
    }
}
impl Executor for Workers {
    fn retire(&mut self, store: &mut Store) -> tl3_controller::Result<()> {
        self.real.retire(store)
    }
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        if matches!(
            delivery.output,
            Out::Effect {
                kind: EffectKind::StartRun(Role::Make) | EffectKind::StopRun,
                ..
            }
        ) {
            self.real.execute(delivery)
        } else {
            let mut result = self.fake.execute(delivery)?;
            Self::plan(&mut result);
            Ok(result)
        }
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        if matches!(
            delivery.output,
            Out::Effect {
                kind: EffectKind::StartRun(Role::Make) | EffectKind::StopRun,
                ..
            }
        ) {
            self.real.reconcile(delivery)
        } else {
            let mut result = self.fake.reconcile(delivery)?;
            if let Reconciliation::Found(completion) = &mut result {
                Self::plan(completion);
            }
            Ok(result)
        }
    }
}

struct Checks {
    fake: adapters::prismabuild::Fake,
    worktree: PathBuf,
    base: String,
    fail: bool,
}
impl Executor for Checks {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        let mut result = self.fake.execute(delivery)?;
        if matches!(
            delivery.output,
            Out::Effect {
                kind: EffectKind::SubmitChecks,
                ..
            }
        ) {
            assert_eq!(
                delivery.head_sha.as_deref(),
                Some(git(&self.worktree, &["rev-parse", "HEAD"]).as_str())
            );
            let changed = git(&self.worktree, &["diff", "--name-only", &self.base, "HEAD"]);
            let inside = changed
                .lines()
                .all(|path| delivery.paths.iter().any(|p| p == path));
            result = Completion::confirmed(vec![Observation::Kernel(Event::Reply {
                node: delivery.node(),
                generation: delivery.generation,
                reply: Reply::ChecksDone {
                    report: true,
                    pass: !self.fail,
                    in_footprint: inside,
                },
            })]);
        }
        Ok(result)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.fake.reconcile(delivery)
    }
}

fn registry(
    config: &tl3_config::Config,
    root: &Path,
    worktree: &Path,
    base: &str,
    fail: bool,
) -> (Registry, WorkerOutputSource) {
    let mut registry = adapters::simulation(root).unwrap();
    let real = Supervisor::new(config, root).unwrap();
    let source = real.source();
    registry.register(
        Upstream::Workers,
        Workers {
            real,
            fake: adapters::workers::Fake::new(root),
        },
    );
    registry.register(
        Upstream::PrismaBuild,
        Checks {
            fake: adapters::prismabuild::Fake::new(root),
            worktree: worktree.into(),
            base: base.into(),
            fail,
        },
    );
    (registry, source)
}

fn requests(config: &tl3_config::Config) -> Vec<Value> {
    let directory = config.workers.runs.join("maker-requests");
    let mut result = fs::read_dir(directory)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("request-")
        })
        .map(|p| serde_json::from_slice::<Value>(&fs::read(p).unwrap()).unwrap())
        .collect::<Vec<_>>();
    result.sort_by_key(|request| request["operation"]["generation"].as_u64());
    result
}

struct CiFailureOnce {
    fake: adapters::mergify::Fake,
    failed: bool,
    details: bool,
}
impl Executor for CiFailureOnce {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        if !self.failed {
            self.failed = true;
            let observation = if self.details {
                Observation::CiFailed {
                    node: delivery.node(),
                    infra: false,
                    checks: vec![tl3_controller::findings::CiFailure {
                        name: "greeting".into(),
                        summary: "test_hello: expected hello, got goodbye".into(),
                        url: Some("https://example.invalid/checks/42".into()),
                    }],
                }
            } else {
                Observation::Kernel(Event::Fact {
                    node: delivery.node(),
                    fact: tl3_kernel::Fact::CiFailed { infra: false },
                })
            };
            Ok(Completion::confirmed(vec![observation]))
        } else {
            self.fake.execute(delivery)
        }
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.fake.reconcile(delivery)
    }
}

fn scenario(settings: &tl3_config::Store, mode: &str, fail: bool) {
    let root = tempfile::tempdir().unwrap();
    let (config, worktree, base) = fixture(settings, root.path(), mode);
    let (mut adapters, source) =
        registry(&config, &root.path().join("fakes"), &worktree, &base, fail);
    if mode.starts_with("ci-") {
        adapters.register(
            Upstream::Mergify,
            CiFailureOnce {
                fake: adapters::mergify::Fake::new(&root.path().join("fakes")),
                failed: false,
                details: mode == "ci-failure",
            },
        );
    }
    let mut controller = Controller::open(
        Store::open(settings).unwrap(),
        adapters,
        Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    let (tx, rx) = mpsc::channel();
    let mut source_thread = Some(thread::spawn(move || source.run(tx)));
    controller.intake(labelled()).unwrap();
    loop {
        let world = controller.world().unwrap();
        if let Some(node) = world.nodes.first()
            && (node.phase.is_closed()
                || (node.replanned && matches!(node.phase, Phase::Planning { .. }))
                || (mode == "out-of-room"
                    && matches!(node.phase, Phase::Planning { must_split: true })))
        {
            break;
        }
        if !controller.drive_once().unwrap() {
            assert!(
                controller
                    .snapshot()
                    .unwrap()
                    .pending_effects
                    .iter()
                    .all(|effect| effect.state != tl3_store::EffectState::Intended),
                "a fixture delivery failed before its worker started"
            );
            let input = rx.recv().unwrap_or_else(|_| {
                panic!(
                    "worker source ended: {:?}",
                    source_thread.take().unwrap().join().unwrap()
                )
            });
            let Input::Observation { key, observation } = input else {
                panic!("worker observation required")
            };
            controller.observe(&key, observation).unwrap();
        }
    }
    let world = controller.world().unwrap();
    if world.nodes[0].phase.is_closed() {
        while controller.drive_once().unwrap() {}
        assert!(
            !config.workers.runs.join("sessions/0").exists(),
            "closed, drained requests must remove every saved conversation"
        );
        // A crash can leave deletion unfinished. Reopening repeats the same safe cleanup.
        let leftover = config.workers.runs.join("sessions/0/plan-0/attempt-1.json");
        fs::create_dir_all(leftover.parent().unwrap()).unwrap();
        fs::write(&leftover, "old conversation").unwrap();
        let mut store = Store::open(settings).unwrap();
        let snapshot = store.load().unwrap();
        let delivery = super::delivery();
        let mut commit = tl3_store::Commit::new("retain-session-watch", json!({}), &snapshot);
        commit.rows.push(tl3_store::RowChange::Put {
            table: "watches".into(),
            id: delivery.key.clone(),
            value: json!(delivery),
        });
        store.commit(&commit).unwrap();
        let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
        supervisor.retire(&mut store).unwrap();
        assert!(
            leftover.exists(),
            "a retained watch can still reconcile a closed node"
        );
        let mut commit =
            tl3_store::Commit::new("drain-session-watch", json!({}), &store.load().unwrap());
        commit.rows.push(tl3_store::RowChange::Delete {
            table: "watches".into(),
            id: delivery.key,
        });
        store.commit(&commit).unwrap();
        drop(controller);
        let (registry, _) = registry(&config, &root.path().join("fakes"), &worktree, &base, fail);
        controller = Controller::open(
            Store::open(settings).unwrap(),
            registry,
            Bootstrap::from_config(&config).unwrap(),
        )
        .unwrap();
        while controller.drive_once().unwrap() {}
        assert!(!leftover.exists());
    } else {
        assert!(
            config.workers.runs.join("sessions/0").exists(),
            "an open request retains its conversations"
        );
    }
    let runs = requests(&config);
    if fail || mode == "outside" {
        assert!(world.nodes[0].replanned);
        assert_eq!(runs.len(), 6);
        for (index, request) in runs.iter().enumerate() {
            let attempt = index / 2 + 1;
            assert_eq!(request["inputs"]["attempt"], attempt);
            assert_eq!(request["inputs"]["round"], index % 2);
            assert_eq!(
                request["model"],
                serde_json::to_value(
                    config
                        .models
                        .worker_model(&config.models.maker[attempt - 1])
                )
                .unwrap()
            );
            if index > 0 {
                assert_eq!(
                    request["inputs"]["feedback"].as_array().unwrap().len(),
                    index
                );
            }
            println!(
                "maker rung attempt {attempt}, round {}: {} effort {} budget {}",
                index % 2,
                request["model"]["name"],
                request["model"]["effort"],
                request["model"]["budget_tokens"]
            );
        }
        let session: Value = serde_json::from_slice(
            &fs::read(config.workers.runs.join("sessions/0/plan-0/attempt-3.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            session
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["type"] == "human")
                .count(),
            2,
            "fix round resumes its attempt session"
        );
        assert!(
            !session.to_string().contains("attempt 1 round"),
            "a new attempt starts a fresh session"
        );
    } else if mode == "cant-meet" || mode == "unmet" {
        assert!(world.nodes[0].replanned);
        assert_eq!(runs.len(), 3);
        assert!(runs.iter().all(|r| r["inputs"]["round"] == 0));
    } else if mode == "out-of-room" {
        assert!(matches!(
            world.nodes[0].phase,
            Phase::Planning { must_split: true }
        ));
        assert_eq!(runs.len(), 1);
    } else {
        assert_eq!(
            world.nodes[0].phase,
            Phase::Closed(tl3_kernel::Outcome::Merged)
        );
        assert_ne!(git(&worktree, &["rev-parse", "HEAD"]), base);
        assert!(
            fs::read_to_string(worktree.join("greeting.py"))
                .unwrap()
                .contains("'hello'")
        );
        if mode.starts_with("ci-") {
            assert_eq!(runs.len(), 2);
            assert_eq!(runs[1]["inputs"]["round"], 1);
            let finding = &runs[1]["inputs"]["feedback"][0];
            assert!(finding["report"].as_str().unwrap().contains("CI"));
            if mode == "ci-failure" {
                assert!(
                    finding["report"]
                        .as_str()
                        .unwrap()
                        .contains("test_hello: expected hello")
                );
                assert!(
                    finding["report"]
                        .as_str()
                        .unwrap()
                        .contains("https://example.invalid/checks/42")
                );
                assert_eq!(finding["ci_checks"][0]["name"], "greeting");
            }
        }
        if mode == "needs-files" {
            assert_eq!(runs.len(), 2);
            assert_eq!(runs[1]["resume_session"], true);
            assert!(
                runs[1]["inputs"]["plan"]["files"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("helper.py"))
            );
            assert!(
                controller
                    .snapshot()
                    .unwrap()
                    .interned
                    .iter()
                    .any(|i| i.kind == tl3_store::InternKind::Path && i.value == "helper.py")
            );
        }
        let reply = Store::open(settings)
            .unwrap()
            .worker_document("0", "maker_replies")
            .unwrap()
            .unwrap();
        assert_eq!(reply["head"], git(&worktree, &["rev-parse", "HEAD"]));
    }
    let snapshot = controller.snapshot().unwrap();
    if !matches!(mode, "cant-meet" | "out-of-room" | "unmet") {
        assert!(
            snapshot
                .interned
                .iter()
                .any(|i| i.kind == tl3_store::InternKind::Commit
                    && i.value == git(&worktree, &["rev-parse", "HEAD"]))
        );
    }
    drop(rx);
    drop(controller);
}

fn process_contract(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let (mut config, worktree, base) = fixture(settings, root.path(), "normal");
    let mut delivery = super::delivery();
    delivery.output = Out::Effect {
        node: NodeId(0),
        seq: 1,
        kind: EffectKind::StartRun(Role::Make),
        head: None,
    };
    delivery.paths = vec!["greeting.py".into(), "test_greeting.py".into()];
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    supervisor.execute(&delivery).unwrap();
    let completion = terminal(&mut supervisor, &delivery);
    assert!(
        matches!(completion.observations.as_slice(), [Observation::WorkerReply {role, reply, ..}] if role == "maker" && reply["kind"] == "done")
    );
    let head = git(&worktree, &["rev-parse", "HEAD"]);
    assert_ne!(head, base);
    drop(supervisor);
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    assert!(matches!(
        supervisor.reconcile(&delivery).unwrap(),
        Reconciliation::Found(_)
    ));
    supervisor.execute(&delivery).unwrap();
    assert_eq!(
        git(&worktree, &["rev-parse", "HEAD"]),
        head,
        "recovery cannot commit twice"
    );
    assert_eq!(attempts(&root_for(&config, &delivery.key)), 1);
    config.workers.command[1] = "bad-schema".into();
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    delivery.key = "maker-invalid".into();
    supervisor.execute(&delivery).unwrap();
    assert!(failed(&terminal(&mut supervisor, &delivery)));
    config.workers.command[1] = "blocked".into();
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    delivery.key = "maker-stop".into();
    supervisor.execute(&delivery).unwrap();
    let attempt = root_for(&config, &delivery.key).join("attempt-00000000");
    wait(|| attempt.join("process.json").exists());
    supervisor.stop_run(&delivery.key, 0).unwrap();
    assert!(!matches!(
        supervisor.reconcile(&delivery).unwrap(),
        Reconciliation::Pending
    ));
}

fn restart_after_replan(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let (mut config, worktree, _) = fixture(settings, root.path(), "needs-files");
    let mut delivery = super::delivery();
    delivery.output = Out::Effect {
        node: NodeId(0),
        seq: 1,
        kind: EffectKind::StartRun(Role::Make),
        head: None,
    };
    delivery.paths = vec!["greeting.py".into(), "test_greeting.py".into()];
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    supervisor.execute(&delivery).unwrap();
    let completion = terminal(&mut supervisor, &delivery);
    assert!(
        matches!(completion.observations.as_slice(), [Observation::WorkerReply { reply, .. }] if reply["kind"] == "needs_files")
    );
    let mut store = Store::open(settings).unwrap();
    let snapshot = store.load().unwrap();
    let mut commit = tl3_store::Commit::new("old-maker-session", json!({}), &snapshot);
    commit.rows.push(tl3_store::RowChange::Put {
        table: "maker_session".into(),
        id: "0".into(),
        value: json!({"attempt": 1, "replanned": false, "needs_files": true}),
    });
    store.commit(&commit).unwrap();
    drop(supervisor);
    delivery.key = "maker-after-replan".into();
    delivery.generation += 1;
    delivery.replanned = true;
    config.workers.command[1] = "replanned".into();
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    supervisor.execute(&delivery).unwrap();
    let completion = terminal(&mut supervisor, &delivery);
    assert!(!failed(&completion));
    let request: Value = serde_json::from_slice(
        &fs::read(root_for(&config, &delivery.key).join("attempt-00000000/request.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(request["inputs"]["attempt"], 1);
    assert_eq!(request["inputs"]["round"], 0);
    assert_eq!(
        request["resume_session"], false,
        "a fitting needs_files from the previous plan cannot resume here"
    );
    let history: Value = serde_json::from_slice(
        &fs::read(config.workers.runs.join("sessions/0/plan-1/attempt-1.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        history
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["type"] == "human")
            .count(),
        1
    );
    assert!(history.to_string().contains("replanned attempt"));
    assert!(!history.to_string().contains("shared greeting is here"));
    assert!(git(&worktree, &["show", "HEAD:greeting.py"]).contains("replanned attempt"));
}

pub(super) fn run(database: &mut Database) {
    real::acceptance(&database.create("maker_real_worktrees"));
    println!("test maker_real_worktrees_and_recorded_harness ... ok");
    restart_after_replan(&database.create("maker_replan_restart"));
    println!("test maker_restart_at_attempt_one_after_replan_starts_fresh ... ok");
    process_contract(&database.create("maker_process"));
    println!("test maker_recovery_invalid_output_and_stop ... ok");
    for (mode, fail) in [
        ("normal", false),
        ("normal", true),
        ("outside", false),
        ("cant-meet", false),
        ("unmet", false),
        ("out-of-room", false),
        ("needs-files", false),
        ("ci-failure", false),
        ("ci-fact", false),
    ] {
        let name = format!("maker_{}_{}", mode.replace('-', "_"), fail);
        scenario(&database.create(&name), mode, fail);
        println!("test maker_{name} ... ok");
    }
}
