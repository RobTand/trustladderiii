use super::*;
use tl3_controller::executor::{Completion, Registry, Upstream};

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

fn fixture(
    settings: &tl3_config::Store,
    root: &Path,
    mode: &str,
) -> (tl3_config::Config, PathBuf, String) {
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
    git(&repository, &["init", "--quiet", "--template="]);
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
    git(&repository, &["add", "."]);
    git(&repository, &["commit", "--quiet", "-m", "Base"]);
    let base = git(&repository, &["rev-parse", "HEAD"]);
    let worktree = config
        .worktrees_root
        .join("RobTand/tl3-sandbox/issue-42-node-0");
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
    Store::open(settings).unwrap().put_worker_materials("0", &json!({"maker": {
        "brief": "Return hello.", "goals": [{"id": "greeting", "description": "Return hello.", "verification": "test"}],
        "plan": {"files": ["not-authoritative.py"], "checks": [{"name": "hello", "kind": "demonstrating", "tests": ["test_greeting.py::test_hello"], "goal_ids": ["greeting"]}]},
        "attempt": 3, "round": 1
    }})).unwrap();
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
    let directory = config.workers.runs.join("sessions/0");
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

fn scenario(settings: &tl3_config::Store, mode: &str, fail: bool) {
    let root = tempfile::tempdir().unwrap();
    let (config, worktree, base) = fixture(settings, root.path(), mode);
    let (registry, source) = registry(&config, &root.path().join("fakes"), &worktree, &base, fail);
    let mut controller = Controller::open(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = source.run(tx);
    });
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
            let Input::Observation { key, observation } = rx.recv().unwrap() else {
                panic!("worker observation required")
            };
            controller.observe(&key, observation).unwrap();
        }
    }
    let world = controller.world().unwrap();
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
            &fs::read(config.workers.runs.join("sessions/0/attempt-3.json")).unwrap(),
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

pub(super) fn run(database: &mut Database) {
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
    ] {
        let name = format!("maker_{}_{}", mode.replace('-', "_"), fail);
        scenario(&database.create(&name), mode, fail);
        println!("test maker_{name} ... ok");
    }
}
