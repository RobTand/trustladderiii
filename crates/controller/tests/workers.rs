#[path = "../../store/tests/support/mod.rs"]
mod support;
use serde_json::{Value, json};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use support::Database;
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue,
    adapters::{
        self,
        workers::{Supervisor, WorkerOutputSource},
    },
    cli::Input,
    executor::{Delivery, Executor, Observation, Reconciliation},
};
use tl3_kernel::{EffectKind, Event, NodeId, Out, Phase, Reply, Role};
use tl3_store::Store;

fn config(settings: &tl3_config::Store, root: &Path, mode: &str) -> tl3_config::Config {
    let mut config = tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
    config.store = settings.clone();
    config.workers.python = worker_package().join(".venv/bin/python");
    config.workers.runs = root.join("runs");
    config.workers.command = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/worker_model.py")
            .to_str()
            .unwrap()
            .into(),
        mode.into(),
        root.join("entered").to_str().unwrap().into(),
    ];
    config.models.key_file = root.join("fixture-key");
    fs::write(&config.models.key_file, "fixture-only").unwrap();
    config.models.base_url = "http://model.invalid/v1".parse().unwrap();
    config
}

fn worker_package() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../workers")
        .canonicalize()
        .unwrap()
}

fn prepare_workers() {
    let uv = env::var_os("TL3_TEST_UV").unwrap_or_else(|| "uv".into());
    let status = Command::new(&uv)
        .args(["sync", "--locked", "--python", "3.14+gil", "--project"])
        .arg(worker_package())
        .status()
        .expect("worker tests require uv 0.10.10: set TL3_TEST_UV or install uv on PATH");
    assert!(
        status.success(),
        "worker venv sync failed; uv must provide Python 3.14 and the locked worker dependencies"
    );
}

fn materials(store: &mut Store) {
    store.put_worker_materials("0", &json!({
        "issue": {"number": 42, "title": "Fix greeting", "body": "Return hello.", "author": "requester", "comments": []},
        "repo": {"readme": "Greeting library", "file_list": ["greeting.py"]},
    })).unwrap();
}

fn labelled() -> LabelledIssue {
    LabelledIssue {
        delivery: "fixture".into(),
        owner: "RobTand".into(),
        repo: "tl3-sandbox".into(),
        installation: 1,
        number: 42,
        actor: 24923402,
        label: "tl3".into(),
    }
}
fn delivery() -> Delivery {
    Delivery {
        key: "effect:0:1".into(),
        epoch: 0,
        output: Out::Effect {
            node: NodeId(0),
            seq: 1,
            kind: EffectKind::StartRun(Role::Ingest),
            head: None,
        },
        generation: 0,
        head_sha: None,
        repository: tl3_controller::Repository {
            owner: "RobTand".into(),
            name: "tl3-sandbox".into(),
            installation: 1,
        },
        issue: 42,
        pull_request: None,
        paths: vec![],
        attempt: 1,
        round: 0,
        note: None,
    }
}
fn root_for(config: &tl3_config::Config, key: &str) -> PathBuf {
    config
        .workers
        .runs
        .join(key.bytes().map(|b| format!("{b:02x}")).collect::<String>())
}
fn wait(mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "fixture did not complete"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
fn terminal(
    supervisor: &mut Supervisor,
    delivery: &Delivery,
) -> tl3_controller::executor::Completion {
    let mut result = None;
    wait(|| match supervisor.reconcile(delivery).unwrap() {
        Reconciliation::Found(completion) => {
            result = Some(completion);
            true
        }
        _ => false,
    });
    result.unwrap()
}
fn failed(completion: &tl3_controller::executor::Completion) -> bool {
    matches!(
        completion.observations.as_slice(),
        [Observation::Kernel(Event::Reply {
            reply: Reply::RunFailed,
            ..
        })]
    )
}

fn main() {
    let args: Vec<_> = env::args().collect();
    if args.get(1).is_some_and(|s| s == "--controller-child") {
        let config_text = fs::read_to_string(&args[2]).unwrap();
        let settings: tl3_config::Store = serde_json::from_str(&config_text).unwrap();
        let root = Path::new(&args[3]);
        let config = config(&settings, root, "blocked");
        let (registry, _) =
            adapters::simulation_with_workers(&root.join("fakes"), &config).unwrap();
        let mut controller = Controller::open(
            Store::open(&settings).unwrap(),
            registry,
            Bootstrap::from_config(&config).unwrap(),
        )
        .unwrap();
        controller.intake(labelled()).unwrap();
        while controller.drive_once().unwrap() {}
        loop {
            thread::park();
        }
    }
    prepare_workers();
    let mut database = Database::start();
    simulation(&database.create("workers_sim"));
    println!("test real_ingest_audit_reaches_unplanned_and_posts_goals ... ok");
    killed_worker(&database.create("workers_kill"));
    println!("test sigkill_worker_retry_is_idempotent_by_key ... ok");
    kernel_retries_killed_worker(&database.create("workers_kernel_retry"));
    println!("test sigkill_worker_failure_retries_through_kernel ... ok");
    late_launcher(&database.create("workers_late"));
    println!("test cancelled_late_launcher_cannot_start_a_second_copy ... ok");
    killed_controller(&database.create("controller_kill"));
    println!("test sigkill_controller_reattaches_to_the_same_worker ... ok");
    invalid_replies(&database.create("workers_invalid"));
    println!("test schema_missing_nonzero_and_duplicate_replies_fail ... ok");
    stop_and_exit_gate(&database.create("workers_stop"));
    println!("test reply_requires_exit_zero_and_stop_waits_for_death ... ok");
    early_and_objections(&mut database);
    println!("test early_end_and_audit_disagreement_mapping ... ok");
    database.stop();
    println!("8 required worker supervisor scenarios passed; no skips");
}

fn drive(controller: &mut Controller, source: &mut WorkerOutputSource, store: &mut Store) {
    let (tx, rx) = mpsc::channel();
    source.poll_once(store, &tx).unwrap();
    while let Ok(Input::Observation { key, observation }) = rx.try_recv() {
        controller.observe(&key, observation).unwrap();
    }
    while controller.drive_once().unwrap() {}
}

fn simulation(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "audit-blocked");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let (registry, mut source) =
        adapters::simulation_with_workers(&root.path().join("fakes"), &config).unwrap();
    let mut controller = Controller::open(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    controller.intake(labelled()).unwrap();
    wait(|| {
        drive(&mut controller, &mut source, &mut store);
        controller.world().unwrap().nodes[0].phase == Phase::Auditing
    });
    // Pause lets the in-flight audit finish and exposes Unplanned before admission.
    controller
        .observe(
            "fixture-pause",
            Observation::Kernel(Event::Command {
                node: NodeId(0),
                id: 1,
                command: tl3_kernel::Command::Pause,
            }),
        )
        .unwrap();
    drive(&mut controller, &mut source, &mut store);
    fs::write(root.path().join("entered.release"), "go").unwrap();
    wait(|| {
        drive(&mut controller, &mut source, &mut store);
        controller.world().unwrap().nodes[0].phase == Phase::Unplanned
    });
    assert_eq!(
        adapters::delivered_notes(&root.path().join("fakes")).unwrap(),
        ["- Return hello."]
    );
    controller
        .observe(
            "fixture-resume",
            Observation::Kernel(Event::Command {
                node: NodeId(0),
                id: 2,
                command: tl3_kernel::Command::Resume,
            }),
        )
        .unwrap();
    wait(|| {
        drive(&mut controller, &mut source, &mut store);
        controller.world().unwrap().nodes[0].phase.is_closed()
    });
    let brief = store.worker_document("0", "briefs").unwrap().unwrap();
    assert_eq!(brief["brief"], "Fix the greeting.");
    assert_eq!(
        store.worker_document("0", "goals").unwrap().unwrap()[0]["description"],
        "Return hello."
    );
    assert_eq!(
        store.worker_document("0", "audits").unwrap().unwrap()["verdict"],
        "agree"
    );
    assert_eq!(
        adapters::delivered_notes(&root.path().join("fakes")).unwrap(),
        ["- Return hello."]
    );
    let progress = store.worker_progress(Some("0")).unwrap();
    assert_eq!(progress.len(), 2);
    assert!(
        progress
            .iter()
            .all(|p| p.last_progress_ms.is_some_and(|ms| ms >= p.started_ms)
                && p.unit == "model_turn")
    );
    let supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let mut replay = supervisor.source();
    let (tx, _rx) = mpsc::channel();
    replay.poll_once(&mut store, &tx).unwrap();
    assert_eq!(
        store.worker_progress(Some("0")).unwrap(),
        progress,
        "replayed progress must not refresh liveness"
    );
    let events = store.events_after(tl3_store::Revision::ZERO, 1000).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.receipt.result["kernel_event"]["Reply"]["reply"] == "AuditAgrees")
    );
    for entry in fs::read_dir(&config.workers.runs).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert_eq!(attempts(&path), 1);
        }
    }
}
fn attempts(root: &Path) -> usize {
    fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.file_name().to_string_lossy().starts_with("attempt-"))
        .count()
}

fn killed_worker(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "blocked");
    materials(&mut Store::open(settings).unwrap());
    let mut supervisor = Supervisor::new(&config, root.path()).unwrap();
    let delivery = delivery();
    supervisor.execute(&delivery).unwrap();
    wait(|| root.path().join("entered").exists());
    let first =
        fs::read(root_for(&config, &delivery.key).join("attempt-00000000/process.json")).unwrap();
    for _ in 0..3 {
        supervisor.execute(&delivery).unwrap();
    }
    assert_eq!(attempts(&root_for(&config, &delivery.key)), 1);
    let pid: Value = serde_json::from_slice(&first).unwrap();
    let status = Command::new("kill")
        .args(["-KILL", &pid["pid"].to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(failed(&terminal(&mut supervisor, &delivery)));
    // A lost terminal record proves absence only after both processes ended.
    fs::remove_file(root_for(&config, &delivery.key).join("attempt-00000000/exit.json")).unwrap();
    assert!(matches!(
        supervisor.reconcile(&delivery).unwrap(),
        Reconciliation::Absent
    ));
    fs::write(root.path().join("entered.release"), "go").unwrap();
    supervisor.execute(&delivery).unwrap();
    supervisor.execute(&delivery).unwrap();
    assert_eq!(attempts(&root_for(&config, &delivery.key)), 2);
    assert!(!failed(&terminal(&mut supervisor, &delivery)));
    assert_eq!(
        fs::read(root_for(&config, &delivery.key).join("attempt-00000000/process.json")).unwrap(),
        first
    );
}

fn killed_controller(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    materials(&mut Store::open(settings).unwrap());
    let settings_path = root.path().join("settings.json");
    fs::write(&settings_path, serde_json::to_vec(&json!({"host": settings.host, "port": settings.port, "database": settings.database, "user": settings.user, "password_file": settings.password_file, "connect_timeout_secs": settings.connect_timeout_secs})).unwrap()).unwrap();
    let mut child = Command::new(env::current_exe().unwrap())
        .arg("--controller-child")
        .arg(settings_path)
        .arg(root.path())
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    wait(|| root.path().join("entered").exists());
    let config = config(settings, root.path(), "blocked");
    let worker_pid = fs::read_to_string(root.path().join("entered")).unwrap();
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    let (registry, mut source) =
        adapters::simulation_with_workers(&root.path().join("fakes"), &config).unwrap();
    let mut controller = Controller::open(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    while controller.drive_once().unwrap() {}
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Ingesting);
    fs::write(root.path().join("entered.release"), "go").unwrap();
    let mut store = Store::open(settings).unwrap();
    wait(|| {
        drive(&mut controller, &mut source, &mut store);
        controller.world().unwrap().nodes[0].phase.is_closed()
    });
    let records: Vec<_> = fs::read_dir(&config.workers.runs)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    assert_eq!(records.len(), 2, "one ingest and one audit");
    assert!(records.iter().all(|p| attempts(p) == 1));
    let ingest = records
        .iter()
        .find(|p| {
            let record: Value =
                serde_json::from_slice(&fs::read(p.join("delivery.json")).unwrap()).unwrap();
            record["output"]["Effect"]["kind"]["StartRun"] == "Ingest"
        })
        .unwrap();
    let identity: Value =
        serde_json::from_slice(&fs::read(ingest.join("attempt-00000000/process.json")).unwrap())
            .unwrap();
    assert_eq!(identity["pid"].to_string(), worker_pid);
    assert!(store.worker_document("0", "briefs").unwrap().is_some());
}

fn invalid_replies(settings: &tl3_config::Store) {
    materials(&mut Store::open(settings).unwrap());
    for mode in [
        "bad-schema",
        "nonzero",
        "missing",
        "duplicate",
        "duplicate-key",
        "wrong-role",
        "malformed",
    ] {
        let root = tempfile::tempdir().unwrap();
        let config = config(settings, root.path(), mode);
        let mut supervisor = Supervisor::new(&config, root.path()).unwrap();
        supervisor.execute(&delivery()).unwrap();
        assert!(failed(&terminal(&mut supervisor, &delivery())), "{mode}");
    }
}

fn stop_and_exit_gate(settings: &tl3_config::Store) {
    materials(&mut Store::open(settings).unwrap());
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "reply-before-exit");
    let mut supervisor = Supervisor::new(&config, root.path()).unwrap();
    let mut run = delivery();
    supervisor.execute(&run).unwrap();
    wait(|| root.path().join("entered").exists());
    assert!(matches!(
        supervisor.reconcile(&run).unwrap(),
        Reconciliation::Pending
    ));
    let process_path = root_for(&config, &run.key).join("attempt-00000000/process.json");
    let process: Value = serde_json::from_slice(&fs::read(&process_path).unwrap()).unwrap();
    run.key = "effect:0:2".into();
    run.output = Out::Effect {
        node: NodeId(0),
        seq: 2,
        kind: EffectKind::StopRun,
        head: None,
    };
    supervisor.execute(&run).unwrap();
    let stat = fs::read_to_string(format!("/proc/{}/stat", process["pid"]));
    assert!(
        stat.is_err()
            || stat
                .unwrap()
                .rsplit_once(')')
                .unwrap()
                .1
                .split_whitespace()
                .next()
                == Some("Z")
    );
    supervisor.execute(&run).unwrap();
    let mut stale = delivery();
    stale.epoch = 1;
    assert!(supervisor.execute(&stale).is_err());
}

fn early_and_objections(database: &mut Database) {
    for mode in ["early", "disagree"] {
        let settings = &database.create(mode);
        materials(&mut Store::open(settings).unwrap());
        let root = tempfile::tempdir().unwrap();
        let config = config(settings, root.path(), mode);
        let (registry, mut source) =
            adapters::simulation_with_workers(&root.path().join("fakes"), &config).unwrap();
        let mut controller = Controller::open(
            Store::open(settings).unwrap(),
            registry,
            Bootstrap::from_config(&config).unwrap(),
        )
        .unwrap();
        controller.intake(labelled()).unwrap();
        let mut store = Store::open(settings).unwrap();
        wait(|| {
            drive(&mut controller, &mut source, &mut store);
            let n = &controller.world().unwrap().nodes[0];
            n.phase.is_closed() || n.question.is_some()
        });
        if mode == "early" {
            assert_eq!(
                controller.world().unwrap().nodes[0].phase,
                Phase::Closed(tl3_kernel::Outcome::Duplicate)
            );
        } else {
            assert_eq!(
                store.worker_document("0", "audits").unwrap().unwrap()["objections"][0],
                "Preserve punctuation."
            );
            let requests: Vec<_> = fs::read_dir(&config.workers.runs)
                .unwrap()
                .map(|e| e.unwrap().path().join("attempt-00000000/request.json"))
                .map(|p| serde_json::from_slice::<Value>(&fs::read(p).unwrap()).unwrap())
                .collect();
            assert!(
                requests
                    .iter()
                    .any(|r| r["inputs"]["audit_objections"][0] == "Preserve punctuation.")
            );
        }
        drop(controller);
    }
}

fn kernel_retries_killed_worker(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "blocked");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let (registry, mut source) =
        adapters::simulation_with_workers(&root.path().join("fakes"), &config).unwrap();
    let mut controller = Controller::open(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    controller.intake(labelled()).unwrap();
    while controller.drive_once().unwrap() {}
    wait(|| root.path().join("entered").exists());
    let pid = fs::read_to_string(root.path().join("entered")).unwrap();
    assert!(
        Command::new("kill")
            .args(["-KILL", &pid])
            .status()
            .unwrap()
            .success()
    );
    fs::write(root.path().join("entered.release"), "go").unwrap();
    wait(|| {
        drive(&mut controller, &mut source, &mut store);
        controller.world().unwrap().nodes[0].phase.is_closed()
    });
    let runs: Vec<_> = fs::read_dir(&config.workers.runs)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    assert_eq!(
        runs.len(),
        3,
        "one failed ingest, its kernel-directed retry, and audit"
    );
    assert!(runs.iter().all(|p| attempts(p) == 1));
}

fn late_launcher(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "normal");
    materials(&mut Store::open(settings).unwrap());
    let mut supervisor = Supervisor::new(&config, root.path()).unwrap();
    let delivery = delivery();
    let attempt = root_for(&config, &delivery.key).join("attempt-00000000");
    fs::create_dir_all(&attempt).unwrap();
    fs::write(
        attempt.join("supervisor.py"),
        include_bytes!("../tools/worker_supervisor.py"),
    )
    .unwrap();
    // This is the record left if the controller dies immediately before spawn.
    assert!(matches!(
        supervisor.reconcile(&delivery).unwrap(),
        Reconciliation::Absent
    ));
    let status = Command::new(&config.workers.python)
        .arg(attempt.join("supervisor.py"))
        .arg(&attempt)
        .status()
        .unwrap();
    assert!(status.success());
    let cancellation: Value =
        serde_json::from_slice(&fs::read(attempt.join("process.json")).unwrap()).unwrap();
    assert_eq!(
        cancellation["cancelled"], true,
        "the delayed worker must retain its cancellation claim"
    );
    assert!(
        !attempt.join("stdout.jsonl").exists(),
        "a cancelled delayed launch must never exec a worker"
    );
    supervisor.execute(&delivery).unwrap();
    assert!(!failed(&terminal(&mut supervisor, &delivery)));
    assert_eq!(attempts(&root_for(&config, &delivery.key)), 2);
}
