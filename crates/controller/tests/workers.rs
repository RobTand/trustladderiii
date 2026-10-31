mod maker;
use tl3_controller::executor::Output as Out;
#[path = "workers/planning.rs"]
mod planning;
mod prismabuild_fixture;
#[path = "workers/retirement.rs"]
mod retirement_review;
mod support;
#[path = "fixtures/waits/mod.rs"]
mod waits;
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{BufRead, BufReader},
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
use tl3_kernel::{EffectKind, Event, NodeId, Phase, Reply, Role};
use tl3_store::{Commit, EffectState, EffectTransition, NewEffect, RowChange, Store};

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
    assert!(
        Command::new(worker_package().join(".venv/bin/python"))
            .args(["-c", "import tiktoken; tiktoken.get_encoding('o200k_base')"])
            .env(
                "TIKTOKEN_CACHE_DIR",
                std::env::temp_dir().join("data-gym-cache")
            )
            .status()
            .unwrap()
            .success()
    );
}

fn materials(store: &mut Store) {
    store.put_worker_materials("0", &json!({
        "issue": {"number": 42, "title": "Fix greeting", "body": "Return hello.", "author": "requester", "comments": []},
        "repo": {"readme": "Greeting library", "file_list": ["greeting.py"], "materials": [{"path": "greeting.py", "content": "def greet(): return 'hello'\n"}]},
    })).unwrap();
}

fn labelled() -> LabelledIssue {
    LabelledIssue {
        delivery: "fixture".into(),
        owner: "RobTand".into(),
        repo: "tl3-sandbox".into(),
        installation: 1,
        number: 42,
        title: "Fix greeting".into(),
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
        replanned: false,
        marker_key: None,
        note: None,
        checks: None,
        job_generation: None,
        question: None,
        question_context: None,
        steering: None,
        request_text: None,
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

fn linked_materials(settings: &tl3_config::Store, wrapped: bool) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "normal");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    if wrapped {
        let inputs = store.worker_materials("0").unwrap().unwrap();
        store
            .put_worker_materials(
                "0",
                &json!({"ingest": inputs, "audit": inputs, "repo": inputs["repo"]}),
            )
            .unwrap();
    }
    let mut commit = Commit::new("linked-materials", json!({}), &store.load().unwrap());
    commit.rows.push(RowChange::Put {
        table: "request_links".into(),
        id: "1".into(),
        value: json!({"original": 0, "issue": {
            "number": 1000, "title": "Correct the greeting", "body": "Repair the delivered result.",
            "state": "open", "updated_at": "2026-10-30T00:00:00Z", "pull_request": null
        }}),
    });
    store.commit(&commit).unwrap();
    let mut delivery = delivery();
    delivery.output = Out::Effect {
        node: NodeId(1),
        seq: 1,
        kind: EffectKind::StartRun(Role::Ingest),
        head: None,
    };
    delivery.issue = 1000;
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    supervisor.execute(&delivery).unwrap();
    assert!(!failed(&terminal(&mut supervisor, &delivery)));
    let request: Value = serde_json::from_slice(
        &fs::read(root_for(&config, &delivery.key).join("attempt-00000000/request.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        request["inputs"]["issue"]["number"], 1000,
        "linked issues must replace the selected role's original issue"
    );
    assert_eq!(request["inputs"]["issue"]["title"], "Correct the greeting");
    assert_eq!(
        request["trace"],
        json!({"repo": "RobTand/tl3-sandbox", "issue": 1000, "request": "1", "attempt": 1, "round": 0}),
        "worker metadata must identify its request without changing role inputs"
    );
    assert_eq!(
        request["inputs"]["issue"]["body"],
        "Repair the delivered result."
    );
    assert_eq!(
        request["inputs"]["repo"]["file_list"],
        json!(["greeting.py"])
    );
}

fn main() {
    if let Some(mode) = env::args().nth(1).filter(|s| s.starts_with("--router-")) {
        maker::router_regression(mode == "--router-look-regression");
        return;
    }

    let args: Vec<_> = env::args().collect();
    if args.get(1).is_some_and(|s| s == "--maker-rebase") {
        prepare_workers();
        let mut database = Database::start();
        for new_edits in [false, true] {
            maker::rebase_trial(
                &database.create(&format!("maker_rebase_{new_edits}")),
                new_edits,
            );
        }
        database.stop();
        return;
    }
    if args.get(1).is_some_and(|s| s == "--source-io-child") {
        let settings = serde_json::from_slice(&fs::read(&args[2]).unwrap()).unwrap();
        retirement_review::io_child(&settings, Path::new(&args[3]));
        return;
    }
    if args.get(1).is_some_and(|s| s == "--source-child") {
        let settings: tl3_config::Store =
            serde_json::from_slice(&fs::read(&args[2]).unwrap()).unwrap();
        let root = Path::new(&args[3]);
        let config = config(&settings, root, "normal");
        let supervisor = Supervisor::new(&config, &root.join("fakes")).unwrap();
        let mut source = supervisor.source();
        let (tx, rx) = mpsc::channel();
        source
            .poll_once(&mut Store::open(&settings).unwrap(), &tx)
            .unwrap();
        fs::write(root.join("source-ready"), "ready").unwrap();
        thread::spawn(move || {
            for input in rx {
                if let Input::Observation { key, observation } = input {
                    println!("{}", json!({"key": key, "observation": observation}));
                }
            }
        });
        source.run(tx).unwrap();
        return;
    }
    if args.get(1).is_some_and(|s| s == "--controller-child") {
        let config_text = fs::read_to_string(&args[2]).unwrap();
        let settings: tl3_config::Store = serde_json::from_str(&config_text).unwrap();
        let root = Path::new(&args[3]);
        let mode = args.get(4).map(String::as_str).unwrap_or("blocked");
        let config = config(&settings, root, mode);
        let (registry, mut source) =
            worker_simulation(&root.join("fakes"), &config, mode != "plan-blocked").unwrap();
        let mut controller = Controller::open(
            Store::open(&settings).unwrap(),
            registry,
            Bootstrap::from_config(&config).unwrap(),
        )
        .unwrap();
        controller.intake(labelled()).unwrap();
        while controller.drive_once().unwrap() {}
        let mut store = Store::open(&settings).unwrap();
        loop {
            if mode == "plan-blocked" {
                drive(&mut controller, &mut source, &mut store);
                thread::sleep(Duration::from_millis(10));
            } else {
                thread::park();
            }
        }
    }
    if args.get(1).is_some_and(|s| s == "--retire-child") {
        let settings: tl3_config::Store =
            serde_json::from_slice(&fs::read(&args[2]).unwrap()).unwrap();
        let root = Path::new(&args[3]);
        let config = config(&settings, root, "normal");
        Supervisor::new(&config, &root.join("fakes"))
            .unwrap()
            .retire(&mut Store::open(&settings).unwrap())
            .unwrap();
        return;
    }
    prepare_workers();
    let mut database = Database::start();
    if args.get(1).is_some_and(|s| s == "--maker-wait") {
        maker::declared_wait(&database.create("maker_declared_wait"));
        println!("test maker_declared_wait_survives_recovery_and_runfailed_retry ... ok");
        database.stop();
        return;
    }
    if args.get(1).is_some_and(|s| s == "--r3") {
        external_wait_survives_restart(&database.create("workers_r3_wait"));
        database.stop();
        return;
    }
    if args.get(1).is_some_and(|s| s == "--linked-materials") {
        for wrapped in [false, true] {
            linked_materials(
                &database.create(&format!("linked_materials_{wrapped}")),
                wrapped,
            );
        }
        database.stop();
        return;
    }
    if args.get(1).is_some_and(|s| s == "--maker-failure") {
        maker::scenario(
            &database.create("maker_failure"),
            &args[2],
            args.get(3).is_some_and(|arg| arg == "--checks-fail"),
        );
        println!("test maker_failure_{} ... ok", args[2]);
        database.stop();
        return;
    }
    if args.get(1).is_some_and(|s| s == "--maker-legacy") {
        maker::legacy_session(&database.create("maker_legacy_session"));
        println!("test maker_legacy_session_fails_closed ... ok");
        database.stop();
        return;
    }
    if args.get(1).is_some_and(|s| s == "--maker-trial") {
        maker::trial(&database.create("maker_real_trial"), Path::new(&args[2]));
        database.stop();
        return;
    }
    if args.get(1).is_some_and(|s| s == "--maker-bounds") {
        maker::bounds_trial(&database.create("maker_outside_bounds"));
        database.stop();
        return;
    }
    if args.get(1).is_some_and(|s| s == "--maker-only") {
        maker::run(&mut database);
        database.stop();
        return;
    }
    if args.get(1).is_some_and(|s| s == "--review-feedback") {
        maker::review_trial(&database.create("review_feedback"));
        database.stop();
        return;
    }
    quota_resumes_same_operation(&database.create("workers_quota"));
    println!("test announced_quota_resumes_same_operation_without_failure ... ok");
    clock_stops_stalled_worker(&database.create("workers_stall"));
    println!("test fake_clock_stops_real_stalled_worker_before_retry ... ok");
    external_wait_survives_restart(&database.create("workers_external_wait"));
    println!("test external_wait_persists_across_source_and_controller_restart ... ok");
    for wrapped in [false, true] {
        linked_materials(
            &database.create(&format!("linked_materials_{wrapped}")),
            wrapped,
        );
        println!("test linked_issue_role_materials_wrapped_{wrapped} ... ok");
    }
    simulation(&database.create("workers_sim"));
    println!("test real_ingest_audit_reaches_unplanned_and_posts_goals ... ok");
    killed_worker(&database.create("workers_kill"));
    println!("test sigkill_worker_retry_is_idempotent_by_key ... ok");
    kernel_retries_killed_worker(&database.create("workers_kernel_retry"));
    println!("test sigkill_worker_failure_retries_through_kernel ... ok");
    late_launcher(&database.create("workers_late"));
    println!("test cancelled_late_launcher_cannot_start_a_second_copy ... ok");
    killed_controller(&database.create("controller_kill"), "blocked");
    killed_controller(&database.create("planner_controller_kill"), "plan-blocked");
    println!("test sigkill_controller_reattaches_to_the_same_worker ... ok");
    invalid_replies(&database.create("workers_invalid"));
    println!("test schema_missing_nonzero_and_duplicate_replies_fail ... ok");
    stop_and_exit_gate(&database.create("workers_stop"));
    println!("test reply_requires_exit_zero_and_stop_waits_for_death ... ok");
    early_and_objections(&mut database);
    println!("test early_end_and_audit_disagreement_mapping ... ok");
    never_started(&database.create("workers_unclaimed"));
    println!("test unclaimed_crash_window_emits_no_failure_and_retries_same_key ... ok");
    partial_crash(&database.create("workers_partial_crash"));
    println!("test partial_last_line_at_sigkill_keeps_only_complete_progress ... ok");
    killed_reply(&database.create("workers_reply_kill"));
    println!("test sigkill_after_valid_reply_is_a_failed_exit ... ok");
    killed_supervisor(&database.create("workers_supervisor_kill"));
    println!("test supervisor_death_keeps_live_worker_and_discards_unrecorded_exit ... ok");
    fake_stop(&database.create("workers_fake_stop"));
    println!("test stop_run_reaches_fake_delivery_and_uses_active_node_index ... ok");
    event_source(&database.create("workers_events"));
    println!("test source_run_wakes_on_new_run_output_and_exit_without_polling ... ok");
    bounded_startup(&database.create("workers_bounded"));
    println!("test startup_reads_only_watched_runs_and_skips_bad_records ... ok");
    retirement(&database.create("workers_retirement"));
    println!("test retirement_requires_terminal_unwatched_effect_and_recovers_from_sigkill ... ok");
    retirement_review::summaries_and_states(&database.create("retirement_summaries"));
    println!("test retirement_states_keep_one_summary_until_closure ... ok");
    retirement_review::blocked_identities(&database.create("retirement_blocked"));
    println!("test retirement_skips_corrupt_and_foreign_identities_after_one_attempt ... ok");
    for guard in ["inbox", "effect", "progress"] {
        retirement_review::session_guard(&database.create(&format!("session_{guard}")), guard);
        println!("test session_retirement_{guard}_guard ... ok");
    }
    retirement_review::session_unlink_error(&database.create("session_unlink"));
    println!("test session_retirement_logs_retains_and_retries_unlink_error ... ok");
    retirement_review::transient_io(&database.create("source_emfile"));
    println!("test live_source_fails_on_emfile_and_restart_reattaches ... ok");
    confirmed_absence(&database.create("workers_confirmed_absence"));
    println!("test confirmed_absence_reports_one_failure_and_retries_with_new_key ... ok");
    overflow(&database.create("workers_overflow"));
    println!("test inotify_overflow_rescans_active_runs_and_keeps_ingress_alive ... ok");
    for mode in [
        "normal",
        "plan-split",
        "plan-invalid",
        "plan-cant-meet",
        "plan-bad-split",
        "plan-bad-schema",
    ] {
        planning::planning(&database.create(&mode.replace('-', "_")), mode);
    }
    println!("test planner_leaf_narrowing_and_rule_retry_with_findings ... ok");
    planning::stop(&database.create("planner_stop"));
    maker::run(&mut database);
    database.stop();
    println!("All required worker supervisor, planning and maker scenarios passed; no skips");
}

// These scenarios isolate ingest/audit. Maker acceptance below uses the real
// supervisor with its own git fixture and model, rather than these role fakes.
fn ingest_audit_simulation(
    root: &Path,
    config: &tl3_config::Config,
) -> tl3_controller::Result<(tl3_controller::executor::Registry, WorkerOutputSource)> {
    worker_simulation(root, config, true)
}

fn worker_simulation(
    root: &Path,
    config: &tl3_config::Config,
    fake_planning: bool,
) -> tl3_controller::Result<(tl3_controller::executor::Registry, WorkerOutputSource)> {
    struct IngestAudit(Supervisor, adapters::workers::Fake, bool);
    impl Executor for IngestAudit {
        fn retire(&mut self, store: &mut Store) -> tl3_controller::Result<()> {
            self.0.retire(store)
        }
        fn execute(
            &mut self,
            delivery: &Delivery,
        ) -> tl3_controller::Result<tl3_controller::executor::Completion> {
            if matches!(
                delivery.output,
                Out::Effect {
                    kind: EffectKind::StartRun(Role::Make | Role::Review),
                    ..
                }
            ) || (self.2
                && matches!(
                    delivery.output,
                    Out::Effect {
                        kind: EffectKind::StartRun(Role::Plan),
                        ..
                    }
                ))
            {
                self.1.execute(delivery)
            } else {
                self.0.execute(delivery)
            }
        }
        fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
            if matches!(
                delivery.output,
                Out::Effect {
                    kind: EffectKind::StartRun(Role::Make | Role::Review),
                    ..
                }
            ) || (self.2
                && matches!(
                    delivery.output,
                    Out::Effect {
                        kind: EffectKind::StartRun(Role::Plan),
                        ..
                    }
                ))
            {
                self.1.reconcile(delivery)
            } else {
                self.0.reconcile(delivery)
            }
        }
    }
    let mut registry = adapters::simulation(root).unwrap();
    let supervisor = Supervisor::new(config, root).unwrap();
    let source = supervisor.source();
    registry.register(
        tl3_controller::executor::Upstream::Workers,
        IngestAudit(
            supervisor,
            adapters::workers::Fake::new(root),
            fake_planning,
        ),
    );
    Ok((registry, source))
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
        ingest_audit_simulation(&root.path().join("fakes"), &config).unwrap();
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
    assert!(progress.is_empty(), "finished progress is retired");
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
    assert_recorded_runs(root.path(), 2);
    assert_retired(&config, &mut store);
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

fn killed_controller(settings: &tl3_config::Store, mode: &str) {
    let root = tempfile::tempdir().unwrap();
    materials(&mut Store::open(settings).unwrap());
    let settings_path = root.path().join("settings.json");
    fs::write(&settings_path, serde_json::to_vec(&json!({"host": settings.host, "port": settings.port, "database": settings.database, "user": settings.user, "password_file": settings.password_file, "connect_timeout_secs": settings.connect_timeout_secs})).unwrap()).unwrap();
    let mut child = Command::new(env::current_exe().unwrap())
        .arg("--controller-child")
        .arg(settings_path)
        .arg(root.path())
        .arg(mode)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    wait(|| root.path().join("entered").exists());
    let config = config(settings, root.path(), mode);
    let worker_pid = fs::read_to_string(root.path().join("entered")).unwrap();
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    let (registry, mut source) =
        worker_simulation(&root.path().join("fakes"), &config, mode != "plan-blocked").unwrap();
    let mut controller = Controller::open(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    while controller.drive_once().unwrap() {}
    if mode == "plan-blocked" {
        assert!(matches!(
            controller.world().unwrap().nodes[0].phase,
            Phase::Planning { .. }
        ));
    } else {
        assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Ingesting);
    }
    let watched = store_worker_run(settings);
    let attempt = root_for(&config, &watched.key).join("attempt-00000000");
    let identity: Value =
        serde_json::from_slice(&fs::read(attempt.join("process.json")).unwrap()).unwrap();
    assert_eq!(identity["pid"].to_string(), worker_pid);
    fs::write(root.path().join("entered.release"), "go").unwrap();
    let mut store = Store::open(settings).unwrap();
    wait(|| {
        drive(&mut controller, &mut source, &mut store);
        controller.world().unwrap().nodes[0].phase.is_closed()
    });
    assert_recorded_runs(root.path(), if mode == "plan-blocked" { 3 } else { 2 });
    assert_retired(&config, &mut store);
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
        "partial",
        "oversized-size",
        "forged-fit",
        "forged-context",
        "forged-overhead",
        "forged-files",
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
            ingest_audit_simulation(&root.path().join("fakes"), &config).unwrap();
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
            let requests: Vec<_> = recorded_runs(root.path())
                .into_iter()
                .map(|r| r["request"].clone())
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
        ingest_audit_simulation(&root.path().join("fakes"), &config).unwrap();
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
    assert_recorded_runs(root.path(), 3);
    assert_retired(&config, &mut store);
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

fn never_started(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "normal");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let run = delivery();
    let run_root = root_for(&config, &run.key);
    let attempt = run_root.join("attempt-00000000");
    fs::create_dir_all(&attempt).unwrap();
    fs::write(
        run_root.join("delivery.json"),
        serde_json::to_vec(&run).unwrap(),
    )
    .unwrap();
    // Controller died between create_dir(attempt) and the launcher's claim.
    let mut source = supervisor.source();
    let (tx, rx) = mpsc::channel();
    source.poll_once(&mut store, &tx).unwrap();
    assert!(matches!(
        supervisor.reconcile(&run).unwrap(),
        Reconciliation::Absent
    ));
    source.poll_once(&mut store, &tx).unwrap();
    assert!(
        rx.try_recv().is_err(),
        "never-started attempts must not charge a failure"
    );
    assert_eq!(source.active_runs(), 0);
    supervisor.execute(&run).unwrap();
    supervisor.execute(&run).unwrap();
    assert!(!failed(&terminal(&mut supervisor, &run)));
    wait(|| {
        source.poll_once(&mut store, &tx).unwrap();
        source.active_runs() == 0
    });
    let messages: Vec<_> = rx.try_iter().collect();
    assert!(matches!(
        messages.last().unwrap(),
        Input::Observation {
            observation: Observation::WorkerReply { .. },
            ..
        }
    ));
    assert!(messages[..messages.len() - 1].iter().all(|input| matches!(
        input,
        Input::Observation {
            observation: Observation::Progress { .. },
            ..
        }
    )));
    assert_eq!(attempts(&run_root), 2);
    fs::write(
        run_root.join("attempt-00000001/stdout.jsonl"),
        "broken history",
    )
    .unwrap();
    source.poll_once(&mut store, &tx).unwrap();
    assert_eq!(
        source.active_runs(),
        0,
        "finished history must remain retired"
    );
    assert!(rx.try_recv().is_err());
}

fn kill_record(path: &Path) {
    let record: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert!(
        Command::new("kill")
            .args(["-KILL", &record["pid"].to_string()])
            .status()
            .unwrap()
            .success()
    );
}

fn killed_reply(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "reply-before-exit");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let run = delivery();
    supervisor.execute(&run).unwrap();
    wait(|| root.path().join("entered").exists());
    let row = &store.worker_progress(None).unwrap()[0];
    assert!(row.started_ms > 0);
    assert_eq!(row.last_progress_ms, None);
    let attempt = root_for(&config, &run.key).join("attempt-00000000");
    let output: Value =
        serde_json::from_slice(&fs::read(attempt.join("stdout.jsonl")).unwrap()).unwrap();
    assert_eq!(output["event"], "reply");
    let mut source = supervisor.source();
    let (tx, rx) = mpsc::channel();
    source.poll_once(&mut store, &tx).unwrap();
    assert!(
        rx.try_recv().is_err(),
        "a reply alone cannot complete a run"
    );
    kill_record(&attempt.join("process.json"));
    assert!(failed(&terminal(&mut supervisor, &run)));
    wait(|| {
        source.poll_once(&mut store, &tx).unwrap();
        source.active_runs() == 0
    });
    assert!(matches!(
        rx.try_recv().unwrap(),
        Input::Observation {
            observation: Observation::Kernel(Event::Reply {
                reply: Reply::RunFailed,
                ..
            }),
            ..
        }
    ));
}

fn killed_supervisor(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "reply-before-exit");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let run = delivery();
    supervisor.execute(&run).unwrap();
    wait(|| root.path().join("entered").exists());
    let attempt = root_for(&config, &run.key).join("attempt-00000000");
    kill_record(&attempt.join("claim.json"));
    // A dead supervisor does not make its still-live worker absent.
    assert!(matches!(
        supervisor.reconcile(&run).unwrap(),
        Reconciliation::Pending
    ));
    let mut source = supervisor.source();
    let (tx, rx) = mpsc::channel();
    source.poll_once(&mut store, &tx).unwrap();
    assert!(rx.try_recv().is_err());
    fs::write(root.path().join("entered.release"), "go").unwrap();
    wait(|| matches!(supervisor.reconcile(&run).unwrap(), Reconciliation::Absent));
    assert!(!attempt.join("exit.json").exists());
    assert!(
        rx.try_recv().is_err(),
        "discard a valid reply without a recorded exit"
    );
    // The retry may start before the source drains the previous pidfd event.
    supervisor.execute(&run).unwrap();
    assert!(!failed(&terminal(&mut supervisor, &run)));
    wait(|| {
        source.poll_once(&mut store, &tx).unwrap();
        source.active_runs() == 0
    });
    let Input::Observation {
        key,
        observation: Observation::WorkerReply { .. },
    } = rx.try_recv().unwrap()
    else {
        panic!("missing retried worker reply")
    };
    assert!(key.ends_with(":worker:1:0"));
    assert!(rx.try_recv().is_err());
    assert_eq!(attempts(&root_for(&config, &run.key)), 2);
}

fn fake_stop(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "blocked");
    let fakes = root.path().join("fakes");
    let mut supervisor = Supervisor::new(&config, &fakes).unwrap();
    materials(&mut Store::open(settings).unwrap());
    let mut run = delivery();
    supervisor.execute(&run).unwrap();
    wait(|| root.path().join("entered").exists());
    let attempt = root_for(&config, &run.key).join("attempt-00000000");
    let process: Value =
        serde_json::from_slice(&fs::read(attempt.join("process.json")).unwrap()).unwrap();
    let pid = rustix::process::Pid::from_raw(process["pid"].as_i64().unwrap().try_into().unwrap())
        .unwrap();
    let fd = rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).unwrap();
    // An unrelated historical record cannot be read by stop_node.
    let history = config.workers.runs.join("old-history");
    fs::create_dir(&history).unwrap();
    fs::write(history.join("delivery.json"), "invalid history").unwrap();
    assert!(
        Supervisor::new(&config, &fakes).is_ok(),
        "startup must skip invalid history"
    );
    run.key = "effect:0:2".into();
    run.output = Out::Effect {
        node: NodeId(0),
        seq: 2,
        kind: EffectKind::StopRun,
        head: None,
    };
    supervisor.execute(&run).unwrap();
    let mut descriptors = [rustix::event::PollFd::new(
        &fd,
        rustix::event::PollFlags::IN,
    )];
    assert_eq!(
        rustix::event::poll(
            &mut descriptors,
            Some(&rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: 0
            })
        )
        .unwrap(),
        1
    );
    assert_eq!(adapters::delivery_counts(&fakes).unwrap()[&run.key], 1);
    assert!(matches!(
        supervisor.reconcile(&run).unwrap(),
        Reconciliation::Found(_)
    ));
}

fn event_source(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "normal");
    let settings_path = root.path().join("settings.json");
    fs::write(&settings_path, serde_json::to_vec(&json!({"host": settings.host, "port": settings.port, "database": settings.database, "user": settings.user, "password_file": settings.password_file, "connect_timeout_secs": settings.connect_timeout_secs})).unwrap()).unwrap();
    materials(&mut Store::open(settings).unwrap());
    let mut child = Command::new(env::current_exe().unwrap())
        .arg("--source-child")
        .arg(&settings_path)
        .arg(root.path())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    wait(|| root.path().join("source-ready").exists());
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut events = Vec::new();
        for line in BufReader::new(stdout).lines() {
            let event: Value = serde_json::from_str(&line.unwrap()).unwrap();
            let progress = event["observation"].get("Progress").is_some();
            events.push(event);
            if !progress {
                tx.send(events).unwrap();
                break;
            }
        }
    });
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    supervisor.execute(&delivery()).unwrap();
    let events = rx.recv_timeout(Duration::from_secs(90));
    child.kill().unwrap();
    child.wait().unwrap();
    reader.join().unwrap();
    let events = events.unwrap();
    let event = events.last().unwrap();
    assert_eq!(event["observation"]["WorkerReply"]["role"], "ingest");
}

fn partial_crash(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "partial-crash");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let run = delivery();
    watch_run(&mut store, &run);
    supervisor.execute(&run).unwrap();
    wait(|| root.path().join("entered").exists());
    let attempt = root_for(&config, &run.key).join("attempt-00000000");
    let bytes = fs::read(attempt.join("stdout.jsonl")).unwrap();
    assert!(!bytes.ends_with(b"\n"));
    let complete = bytes.iter().position(|b| *b == b'\n').unwrap() + 1;
    let mut source = supervisor.source();
    let (tx, rx) = mpsc::channel();
    source.poll_once(&mut store, &tx).unwrap();
    assert_eq!(
        store.worker_progress(None).unwrap()[0].output_offset,
        complete as i64
    );
    assert!(matches!(rx.try_recv().unwrap(), Input::Observation {
        observation: Observation::Progress { checkpoint, .. }, ..
    } if checkpoint == format!("0:{complete}")));
    assert!(
        rx.try_recv().is_err(),
        "a partial reply cannot finish the run"
    );
    let progress = store.worker_progress(None).unwrap();
    assert!(
        progress[0]
            .last_progress_ms
            .is_some_and(|ms| ms >= progress[0].started_ms)
    );
    assert_eq!(progress[0].unit, "model_turn");
    let mut replay = Supervisor::new(&config, &root.path().join("fakes"))
        .unwrap()
        .source();
    replay.poll_once(&mut store, &tx).unwrap();
    assert_eq!(
        store.worker_progress(None).unwrap(),
        progress,
        "restart replay cannot refresh progress time"
    );
    assert!(rx.try_recv().is_err());
    kill_record(&attempt.join("process.json"));
    assert!(failed(&terminal(&mut supervisor, &run)));
    wait(|| {
        source.poll_once(&mut store, &tx).unwrap();
        source.active_runs() == 0
    });
    assert_eq!(
        store.worker_progress(None).unwrap()[0].output_offset,
        complete as i64
    );
    assert!(matches!(
        rx.try_recv().unwrap(),
        Input::Observation {
            observation: Observation::Kernel(Event::Reply {
                reply: Reply::RunFailed,
                ..
            }),
            ..
        }
    ));
}

fn clock_stops_stalled_worker(settings: &tl3_config::Store) {
    #[derive(Clone)]
    struct Clock(std::rc::Rc<std::cell::Cell<u64>>);
    impl tl3_controller::clock::Clock for Clock {
        fn now(&self) -> std::time::SystemTime {
            std::time::UNIX_EPOCH + Duration::from_millis(self.0.get())
        }
    }
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "blocked");
    materials(&mut Store::open(settings).unwrap());
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let mut registry = adapters::simulation(&root.path().join("fakes")).unwrap();
    registry.register(
        tl3_controller::executor::Upstream::Workers,
        supervisor.clone(),
    );
    let clock = Clock(std::rc::Rc::new(std::cell::Cell::new(1000)));
    let mut controller = Controller::open_with_clock(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap::from_config(&config).unwrap(),
        clock.clone(),
    )
    .unwrap();
    let mut allowances = config.allowances.clone();
    allowances.agent_worker.silence_secs = 1;
    allowances.agent_worker.no_progress_secs = 3;
    controller.set_allowances(allowances);
    controller.intake(labelled()).unwrap();
    while controller.drive_once().unwrap() {}
    wait(|| root.path().join("entered").exists());
    let snapshot = controller.snapshot().unwrap();
    let original: Delivery = serde_json::from_value(
        snapshot
            .rows
            .iter()
            .find(|r| r.table == "watches")
            .unwrap()
            .value
            .clone(),
    )
    .unwrap();
    controller.look_now().unwrap();
    clock.0.set(4000);
    controller.look_due().unwrap();
    while controller.drive_once().unwrap() {}
    let node = controller.world().unwrap().nodes[0];
    assert_eq!(node.run_failures, 1);
    assert!(node.generation > original.generation);
    assert!(
        !matches!(
            supervisor.reconcile(&original).unwrap(),
            Reconciliation::Pending
        ),
        "retry must start only after the original process ended"
    );
    controller.look_now().unwrap();
    clock.0.set(7000);
    controller.look_due().unwrap();
    while controller.drive_once().unwrap() {}
    assert!(controller.world().unwrap().nodes[0].waiting_incident);
    clock.0.set(8000);
    controller.look_due().unwrap();
    while controller.drive_once().unwrap() {}
    assert!(
        !controller.world().unwrap().nodes[0].waiting_incident,
        "a passing worker runtime probe clears the first incident"
    );
    controller
        .observe(
            "stop-after-stall",
            Observation::Kernel(Event::Command {
                node: NodeId(0),
                id: 1,
                command: tl3_kernel::Command::Stop,
            }),
        )
        .unwrap();
    while controller.drive_once().unwrap() {}
    assert!(matches!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(_)
    ));
}

fn quota_resumes_same_operation(settings: &tl3_config::Store) {
    #[derive(Clone)]
    struct Clock(std::rc::Rc<std::cell::Cell<u64>>);
    impl tl3_controller::clock::Clock for Clock {
        fn now(&self) -> std::time::SystemTime {
            std::time::UNIX_EPOCH + Duration::from_millis(self.0.get())
        }
    }
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "quota-once");
    materials(&mut Store::open(settings).unwrap());
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let mut registry = adapters::simulation(&root.path().join("fakes")).unwrap();
    registry.register(
        tl3_controller::executor::Upstream::Workers,
        supervisor.clone(),
    );
    let initial = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let clock = Clock(std::rc::Rc::new(std::cell::Cell::new(initial)));
    let mut controller = Controller::open_with_clock(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap::from_config(&config).unwrap(),
        clock.clone(),
    )
    .unwrap();
    controller.intake(labelled()).unwrap();
    while controller.drive_once().unwrap() {}
    let snapshot = controller.snapshot().unwrap();
    let original: Delivery = serde_json::from_value(
        snapshot
            .rows
            .iter()
            .find(|r| r.table == "watches")
            .unwrap()
            .value
            .clone(),
    )
    .unwrap();
    let completion = terminal(&mut supervisor, &original);
    let reset = match completion.observations.as_slice() {
        [Observation::Quota { reset_ms, .. }] => *reset_ms,
        other => panic!("expected quota ending: {other:?}"),
    };
    while controller.drive_once().unwrap() {}
    clock.0.set(reset - 1);
    controller.look_due().unwrap();
    while controller.drive_once().unwrap() {}
    assert_eq!(
        Store::open(settings)
            .unwrap()
            .worker_progress(None)
            .unwrap()[0]
            .attempt,
        0
    );
    clock.0.set(reset);
    controller.look_due().unwrap();
    let node = controller.world().unwrap().nodes[0];
    assert_eq!(node.run_failures, 0);
    assert_eq!(node.generation, original.generation);
    assert_eq!(
        Store::open(settings)
            .unwrap()
            .worker_progress(None)
            .unwrap()[0]
            .attempt,
        1
    );
    wait(|| {
        for iteration in 0..200 {
            if !controller.drive_once().unwrap() {
                break;
            }
            assert!(iteration < 199, "quota result replay did not settle");
        }
        controller.world().unwrap().nodes[0].phase != Phase::Ingesting
    });
    assert_eq!(controller.world().unwrap().nodes[0].run_failures, 0);
    controller
        .observe(
            "stop-after-quota",
            Observation::Kernel(Event::Command {
                node: NodeId(0),
                id: 1,
                command: tl3_kernel::Command::Stop,
            }),
        )
        .unwrap();
    while controller.drive_once().unwrap() {}
    assert!(matches!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(_)
    ));
}

fn recorded_runs(root: &Path) -> Vec<Value> {
    fs::read_to_string(root.join("entered.requests"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn assert_recorded_runs(root: &Path, count: usize) {
    let records = recorded_runs(root);
    assert_eq!(records.len(), count);
    let keys: std::collections::BTreeSet<_> = records
        .iter()
        .map(|r| r["request"]["worktree"].as_str().unwrap())
        .collect();
    assert_eq!(keys.len(), count, "each key launched exactly one worker");
}

fn assert_retired(config: &tl3_config::Config, store: &mut Store) {
    assert!(store.worker_progress(None).unwrap().is_empty());
    assert!(store.worker_watches().unwrap().is_empty());
    assert_eq!(
        fs::read_dir(&config.workers.runs).unwrap().count(),
        1,
        "only the notification inbox remains"
    );
    assert_eq!(
        fs::read_dir(config.workers.runs.join(".ready"))
            .unwrap()
            .count(),
        0
    );
}

fn store_worker_run(settings: &tl3_config::Store) -> Delivery {
    serde_json::from_value(
        Store::open(settings).unwrap().worker_watches().unwrap()[0]
            .1
            .clone(),
    )
    .unwrap()
}

fn save(
    store: &mut Store,
    rows: Vec<RowChange>,
    transitions: Vec<EffectTransition>,
    effects: Vec<NewEffect>,
) {
    let snapshot = store.load().unwrap();
    let mut commit = Commit::new(
        format!("fixture:{}", snapshot.revision.get()),
        json!({"fixture": snapshot.revision.get()}),
        &snapshot,
    );
    commit.rows = rows;
    commit.transitions = transitions;
    commit.effects = effects;
    store.commit(&commit).unwrap();
}

fn watch_run(store: &mut Store, run: &Delivery) {
    save(
        store,
        vec![RowChange::Put {
            table: "watches".into(),
            id: run.key.clone(),
            value: json!(run),
        }],
        vec![],
        vec![NewEffect {
            key: run.key.clone(),
            kind: "Workers".into(),
            payload: json!(run),
        }],
    );
}

fn unwatch(store: &mut Store, run: &Delivery) {
    save(
        store,
        vec![RowChange::Delete {
            table: "watches".into(),
            id: run.key.clone(),
        }],
        vec![],
        vec![],
    );
}

fn bounded_startup(settings: &tl3_config::Store) {
    use rustix::fs::inotify;
    use std::mem::MaybeUninit;
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "blocked");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let run = delivery();
    watch_run(&mut store, &run);
    let mut original = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    original.execute(&run).unwrap();
    wait(|| root.path().join("entered").exists());
    let observer = inotify::init(inotify::CreateFlags::NONBLOCK).unwrap();
    for n in 0..128 {
        let history = config.workers.runs.join(format!("history-{n}"));
        let attempt = history.join("attempt-00000000");
        fs::create_dir_all(&attempt).unwrap();
        fs::write(history.join("delivery.json"), "invalid historical delivery").unwrap();
        fs::write(
            attempt.join("claim.json"),
            json!({"pid": 1, "start": 1, "boot": "old", "machine": "another-machine"}).to_string(),
        )
        .unwrap();
        fs::write(attempt.join("exit.json"), "{\"code\":0}").unwrap();
        fs::write(attempt.join("stdout.jsonl"), "invalid historical output").unwrap();
        for path in [&history, &attempt] {
            inotify::add_watch(
                &observer,
                path,
                inotify::WatchFlags::OPEN | inotify::WatchFlags::ACCESS,
            )
            .unwrap();
        }
    }
    let mut finished = run.clone();
    finished.key = "effect:finished:1".into();
    watch_run(&mut store, &finished);
    save(
        &mut store,
        vec![RowChange::Delete {
            table: "watches".into(),
            id: finished.key.clone(),
        }],
        vec![
            EffectTransition {
                key: finished.key.clone(),
                from: EffectState::Intended,
                to: EffectState::Sent,
            },
            EffectTransition {
                key: finished.key.clone(),
                from: EffectState::Sent,
                to: EffectState::Confirmed,
            },
        ],
        vec![],
    );
    let finished_root = root_for(&config, &finished.key);
    fs::create_dir(&finished_root).unwrap();
    fs::write(finished_root.join("delivery.json"), "bad finished record").unwrap();
    let finished_ready = config.workers.runs.join(".ready").join(format!(
        ".ready-{}",
        finished_root.file_name().unwrap().to_string_lossy()
    ));
    fs::write(&finished_ready, json!(finished).to_string()).unwrap();
    inotify::add_watch(
        &observer,
        &finished_root,
        inotify::WatchFlags::OPEN | inotify::WatchFlags::ACCESS,
    )
    .unwrap();
    let mut bad = run.clone();
    bad.key = "effect:bad:1".into();
    watch_run(&mut store, &bad);
    let bad_attempt = root_for(&config, &bad.key).join("attempt-00000000");
    fs::create_dir_all(&bad_attempt).unwrap();
    fs::write(bad_attempt.join("claim.json"), "broken active record").unwrap();
    let mut foreign = run.clone();
    foreign.key = "effect:foreign:1".into();
    watch_run(&mut store, &foreign);
    let foreign_attempt = root_for(&config, &foreign.key).join("attempt-00000000");
    fs::create_dir_all(&foreign_attempt).unwrap();
    fs::write(
        foreign_attempt.join("claim.json"),
        json!({"pid": 1, "start": 1, "boot": "old", "machine": "another-machine"}).to_string(),
    )
    .unwrap();
    let name = root_for(&config, &run.key).file_name().unwrap().to_owned();
    let legacy = config
        .workers
        .runs
        .join(format!(".ready-{}", name.to_string_lossy()));
    fs::write(&legacy, "stale legacy notification").unwrap();
    let corrupt_ready = config.workers.runs.join(".ready/.ready-deadbeef");
    fs::write(&corrupt_ready, "broken notification").unwrap();
    let mut restarted = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let mut source = restarted.source();
    let (tx, rx) = mpsc::channel();
    source.poll_once(&mut store, &tx).unwrap();
    assert_eq!(
        source.active_runs(),
        1,
        "bad and foreign records are skipped independently"
    );
    assert!(!legacy.exists());
    assert!(!corrupt_ready.exists());
    assert!(!finished_ready.exists());
    assert!(rx.try_recv().is_err());
    let mut buffer = [MaybeUninit::uninit(); 1024];
    let mut reader = inotify::Reader::new(&observer, &mut buffer);
    assert!(
        matches!(reader.next(), Err(rustix::io::Errno::AGAIN)),
        "startup must not open any history directory or file"
    );
    let mut stop = run.clone();
    stop.key = "stop:0".into();
    stop.output = Out::Effect {
        node: NodeId(0),
        seq: 2,
        kind: EffectKind::StopRun,
        head: None,
    };
    restarted.execute(&stop).unwrap();
    assert!(
        matches!(
            original.reconcile(&run).unwrap(),
            Reconciliation::Absent | Reconciliation::Found(_)
        ),
        "restarted ActiveRuns must find and stop the live worker"
    );
}

fn retirement(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "blocked");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let run = delivery();
    watch_run(&mut store, &run);
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    supervisor.execute(&run).unwrap();
    wait(|| root.path().join("entered").exists());
    let mut source = supervisor.source();
    let (tx, rx) = mpsc::channel();
    source.poll_once(&mut store, &tx).unwrap();
    assert_eq!(source.active_runs(), 1);
    let run_root = root_for(&config, &run.key);
    unwatch(&mut store, &run);
    supervisor.retire(&mut store).unwrap();
    assert!(
        run_root.exists(),
        "pending effects retain their evidence without a watch"
    );
    save(
        &mut store,
        vec![RowChange::Put {
            table: "watches".into(),
            id: run.key.clone(),
            value: json!(run),
        }],
        vec![
            EffectTransition {
                key: run.key.clone(),
                from: EffectState::Intended,
                to: EffectState::Sent,
            },
            EffectTransition {
                key: run.key.clone(),
                from: EffectState::Sent,
                to: EffectState::Confirmed,
            },
        ],
        vec![],
    );
    supervisor.retire(&mut store).unwrap();
    assert!(
        run_root.exists(),
        "a confirmed effect remains reconcilable while watched"
    );
    assert_eq!(store.worker_progress(None).unwrap().len(), 1);
    unwatch(&mut store, &run);
    // Hold only the final DB deletion. The child can remove files but cannot forget its cleanup row.
    let mut connection = postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .dbname(&settings.database)
        .user(&settings.user)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap();
    let mut transaction = connection.transaction().unwrap();
    transaction
        .query_one(
            "SELECT run_key FROM tl3_worker_progress WHERE run_key = $1 FOR UPDATE",
            &[&run.key],
        )
        .unwrap();
    let settings_path = root.path().join("settings.json");
    fs::write(&settings_path, serde_json::to_vec(&json!({"host": settings.host, "port": settings.port, "database": settings.database, "user": settings.user, "password_file": settings.password_file, "connect_timeout_secs": settings.connect_timeout_secs})).unwrap()).unwrap();
    let mut child = Command::new(env::current_exe().unwrap())
        .arg("--retire-child")
        .arg(&settings_path)
        .arg(root.path())
        .spawn()
        .unwrap();
    wait(|| !run_root.exists());
    let mut backend = None;
    wait(|| {
        transaction
            .simple_query("SELECT pg_stat_clear_snapshot()")
            .unwrap();
        backend = transaction.query_opt("SELECT pid FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock' AND query LIKE 'DELETE FROM tl3_worker_progress%'", &[]).unwrap().map(|row| row.get::<_, i32>(0));
        backend.is_some()
    });
    assert!(
        child.try_wait().unwrap().is_none(),
        "cleanup is blocked at the final row deletion"
    );
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    // Client death does not cancel an executing SQL statement. End that session too.
    let backend = backend.unwrap();
    transaction
        .query(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE pid = $1",
            &[&backend],
        )
        .unwrap();
    wait(|| {
        transaction
            .simple_query("SELECT pg_stat_clear_snapshot()")
            .unwrap();
        !transaction
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE pid = $1)",
                &[&backend],
            )
            .unwrap()
            .get::<_, bool>(0)
    });
    transaction.rollback().unwrap();
    assert_eq!(
        store.worker_progress(None).unwrap().len(),
        1,
        "SIGKILL retains the durable cleanup queue"
    );
    source.poll_once(&mut store, &tx).unwrap();
    assert_eq!(
        source.active_runs(),
        0,
        "retirement drops source watches even when files disappear before refresh"
    );
    assert!(rx.try_recv().is_err());
    let mut restarted = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    restarted.retire(&mut store).unwrap();
    restarted.retire(&mut store).unwrap();
    assert_retired(&config, &mut store);

    // A source can attach before an attempt exists, with no pidfd to wake it.
    fs::create_dir_all(&run_root).unwrap();
    store.claim_worker(&run.key, "0", 0, 0).unwrap();
    save(
        &mut store,
        vec![RowChange::Put {
            table: "watches".into(),
            id: run.key.clone(),
            value: json!(run),
        }],
        vec![],
        vec![],
    );
    let mut unclaimed_source = restarted.source();
    unclaimed_source.poll_once(&mut store, &tx).unwrap();
    assert_eq!(unclaimed_source.active_runs(), 1);
    unwatch(&mut store, &run);
    restarted.retire(&mut store).unwrap();
    unclaimed_source.poll_once(&mut store, &tx).unwrap();
    assert_eq!(
        unclaimed_source.active_runs(),
        0,
        "directory removal wakes a source without process identities"
    );
    assert!(rx.try_recv().is_err());
    assert_retired(&config, &mut store);
}

fn kill_record_and_wait(path: &Path) {
    use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
    let record: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let fd = pidfd_open(
        Pid::from_raw(record["pid"].as_i64().unwrap().try_into().unwrap()).unwrap(),
        PidfdFlags::empty(),
    )
    .unwrap();
    pidfd_send_signal(&fd, Signal::KILL).unwrap();
    let mut descriptors = [rustix::event::PollFd::new(
        &fd,
        rustix::event::PollFlags::IN,
    )];
    loop {
        match rustix::event::poll(&mut descriptors, None) {
            Ok(_) => break,
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => panic!("pidfd wait failed: {error}"),
        }
    }
}

fn confirmed_absence(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "blocked");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let (registry, mut source) =
        ingest_audit_simulation(&root.path().join("fakes"), &config).unwrap();
    let mut controller = Controller::open(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    controller.intake(labelled()).unwrap();
    while controller.drive_once().unwrap() {}
    wait(|| root.path().join("entered").exists());
    let run = store_worker_run(settings);
    assert_eq!(
        store.effect(&run.key).unwrap().unwrap().state,
        EffectState::Confirmed
    );
    let attempt = root_for(&config, &run.key).join("attempt-00000000");
    let (tx, rx) = mpsc::channel();
    source.poll_once(&mut store, &tx).unwrap();
    kill_record_and_wait(&attempt.join("claim.json"));
    kill_record_and_wait(&attempt.join("process.json"));
    wait(|| {
        source.poll_once(&mut store, &tx).unwrap();
        source.active_runs() == 0
    });
    assert!(!attempt.join("exit.json").exists());
    assert!(
        rx.try_recv().is_err(),
        "source emits nothing for confirmed absence"
    );
    while controller.drive_once().unwrap() {}
    let replacement = store_worker_run(settings);
    assert_ne!(
        replacement.key, run.key,
        "the kernel retries under a new effect key"
    );
    let failures = |store: &mut Store| {
        store
            .events_after(tl3_store::Revision::ZERO, 1000)
            .unwrap()
            .iter()
            .filter(|e| e.receipt.result["kernel_event"]["Reply"]["reply"] == "RunFailed")
            .count()
    };
    assert_eq!(failures(&mut store), 1);
    fs::write(root.path().join("entered.release"), "go").unwrap();
    wait(|| {
        drive(&mut controller, &mut source, &mut store);
        controller.world().unwrap().nodes[0].phase.is_closed()
    });
    assert_eq!(
        failures(&mut store),
        1,
        "later looks cannot charge the missing run again"
    );
    assert_recorded_runs(root.path(), 3);
    assert_retired(&config, &mut store);
}

fn overflow(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "blocked");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let mut supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let mut run = delivery();
    supervisor.execute(&run).unwrap();
    wait(|| root.path().join("entered").exists());
    let mut source = supervisor.source();
    let (tx, rx) = mpsc::channel();
    source.poll_once(&mut store, &tx).unwrap();
    let limit: usize = fs::read_to_string("/proc/sys/fs/inotify/max_queued_events")
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let noise = root_for(&config, &run.key).join("attempt-00000000/noise");
    for _ in 0..=limit {
        fs::write(&noise, "noise").unwrap();
    }
    run.key = "effect:0:2".into();
    supervisor.execute(&run).unwrap();
    source.poll_once(&mut store, &tx).unwrap();
    assert_eq!(
        source.recovered_overflows(),
        1,
        "the kernel queue really overflowed"
    );
    assert_eq!(
        source.active_runs(),
        2,
        "overflow rescan discovers a dropped ready event"
    );
    fs::write(root.path().join("entered.release"), "go").unwrap();
    wait(|| {
        source.poll_once(&mut store, &tx).unwrap();
        source.active_runs() == 0
    });
    let events: Vec<_> = rx.try_iter().collect();
    assert_eq!(
        events
            .iter()
            .filter(|input| matches!(
                input,
                Input::Observation {
                    observation: Observation::WorkerReply { .. },
                    ..
                }
            ))
            .count(),
        2,
        "overflow recovery emits each reply once"
    );
    assert_eq!(
        events
            .iter()
            .filter(|input| matches!(
                input,
                Input::Observation {
                    observation: Observation::Progress { .. },
                    ..
                }
            ))
            .count(),
        2,
        "each run retains its progress observation"
    );
    let keys: std::collections::BTreeSet<_> = events
        .iter()
        .filter_map(|input| match input {
            Input::Observation { key, .. } => Some(key),
            _ => None,
        })
        .collect();
    assert_eq!(
        keys.len(),
        events.len(),
        "overflow recovery never replays an event"
    );
}

fn external_wait_survives_restart(settings: &tl3_config::Store) {
    #[derive(Clone)]
    struct Clock(std::rc::Rc<std::cell::Cell<u64>>);
    impl tl3_controller::clock::Clock for Clock {
        fn now(&self) -> std::time::SystemTime {
            std::time::UNIX_EPOCH + Duration::from_millis(self.0.get())
        }
    }
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "external-wait");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let mut source = supervisor.source();
    let clock = Clock(std::rc::Rc::new(std::cell::Cell::new(1000)));
    let open = || {
        let mut registry = adapters::simulation(&root.path().join("fakes")).unwrap();
        registry.register(
            tl3_controller::executor::Upstream::Workers,
            supervisor.clone(),
        );
        registry.register(
            tl3_controller::executor::Upstream::PrismaBuild,
            waits::adapter(root.path()),
        );
        let mut c = Controller::open_with_clock(
            Store::open(settings).unwrap(),
            registry,
            Bootstrap::from_config(&config).unwrap(),
            clock.clone(),
        )
        .unwrap();
        let mut allowances = config.allowances.clone();
        allowances.agent_worker.silence_secs = 1;
        allowances.agent_worker.no_progress_secs = 3;
        c.set_allowances(allowances);
        c
    };
    let mut c = open();
    waits::record(root.path(), "claimed");
    c.intake(labelled()).unwrap();
    while c.drive_once().unwrap() {}
    wait(|| root.path().join("entered").exists());
    let (tx, rx) = mpsc::channel();
    wait(|| {
        source.poll_once(&mut store, &tx).unwrap();
        store
            .worker_progress(None)
            .unwrap()
            .iter()
            .any(|r| r.external_wait.is_some())
    });
    assert!(
        rx.try_iter()
            .all(|input| matches!(input, Input::Look { .. })),
        "a declaration requests a look without claiming agent progress"
    );
    let mut before = store.worker_progress(None).unwrap();
    let run = &before[0];
    let encoded: String = run.key.bytes().map(|b| format!("{b:02x}")).collect();
    let record_path = root
        .path()
        .join(format!("queue/claimed/{}.json", waits::key()));
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    record["checkout_snapshot"]["parent"] = json!(waits::bind(
        root.path(),
        &config.workers.runs.join(&encoded).join("worktree")
    ));
    fs::write(record_path, record.to_string()).unwrap();
    c.look_now().unwrap();
    let before_repeat = c.snapshot().unwrap();
    fs::write(root.path().join("entered.repeat"), "repeat").unwrap();
    wait(|| {
        source.poll_once(&mut store, &tx).unwrap();
        store.worker_progress(None).unwrap()[0].output_offset > before[0].output_offset
    });
    assert!(
        rx.try_recv().is_err(),
        "reattaching the same wait must not request another PrismaBuild look"
    );
    assert_eq!(
        c.snapshot().unwrap(),
        before_repeat,
        "reattachment does not commit allowance state"
    );
    before = store.worker_progress(None).unwrap();
    c.look_now().unwrap();
    drop(c);
    drop(source);
    let mut source = supervisor.source();
    let mut c = open();
    source.poll_once(&mut store, &tx).unwrap();
    assert_eq!(store.worker_progress(None).unwrap(), before);
    assert!(rx.try_recv().is_err(), "replaying a wait is not progress");
    clock.0.set(1_000_000);
    c.look_due().unwrap();
    while c.drive_once().unwrap() {}
    assert_eq!(
        c.world().unwrap().nodes[0].run_failures,
        0,
        "claimed action survives after driving RunFailed"
    );
    assert!(
        !c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "allowances" && r.value["stopping"] == true),
        "claimed action never enters stop_stalled"
    );
    let attempt = config
        .workers
        .runs
        .join(&encoded)
        .join(format!("attempt-{:08}", before[0].attempt));
    let claim: serde_json::Value =
        serde_json::from_slice(&fs::read(attempt.join("claim.json")).unwrap()).unwrap();
    assert_ne!(
        claim["cancelled"], true,
        "claimed action has no cancellation"
    );
    assert!(
        !attempt.join("exit.json").exists(),
        "claimed action has not been stopped"
    );
    waits::record(root.path(), "done");
    c.look_now().unwrap();
    clock.0.set(1_003_000);
    c.look_due().unwrap();
    assert!(c.drive_once().unwrap());
    assert_eq!(c.world().unwrap().nodes[0].run_failures, 1);
}
