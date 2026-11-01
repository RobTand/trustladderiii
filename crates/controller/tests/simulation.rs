use serde_json::json;
use tl3_controller::executor::Output as Out;
#[path = "../../store/tests/support/mod.rs"]
mod support;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
use support::Database;
use tl3_controller::{
    Bootstrap, Boundary, Controller, LabelledIssue, Repository, adapters,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation, Registry, Upstream},
    status,
};
use tl3_kernel::{Ack, Command as Control, EffectKind, Event, Fact, NodeId, Outcome, Phase};
use tl3_store::{Commit, EffectState, EffectTransition, Epoch, Reader, RowChange, Snapshot, Store};

fn bootstrap() -> Bootstrap {
    Bootstrap {
        operations: None,
        repos: vec![Repository {
            owner: "RobTand".into(),
            name: "tl3-sandbox".into(),
            installation: 1,
        }],
        rob: 24923402,
        slots: 1,
        pool: 1,
    }
}
fn labelled() -> LabelledIssue {
    LabelledIssue {
        delivery: "fixture-delivery".into(),
        owner: "RobTand".into(),
        repo: "tl3-sandbox".into(),
        installation: 1,
        number: 42,
        title: "Fix the sandbox build".into(),
        actor: 24923402,
        label: "tl3".into(),
    }
}
fn open(settings: &tl3_config::Store, root: &Path) -> Controller {
    Controller::open(
        Store::open(settings).unwrap(),
        adapters::simulation(root).unwrap(),
        bootstrap(),
    )
    .unwrap()
}

#[path = "simulation/gate_migration.rs"]
mod gate_migration;

fn main() {
    let args: Vec<_> = env::args().collect();
    if args.get(1).is_some_and(|s| s == "--child") {
        child(&args);
        return;
    }
    if args.get(1).is_some_and(|s| s == "--gate-migration-child") {
        gate_migration::child(&args);
        return;
    }
    let mut database = Database::start();
    type Scenario = (&'static str, fn(&tl3_config::Store));
    let cases: &[Scenario] = &[
        ("agent_capacity_reopen", gate_migration::pool_reopen),
        ("agent_capacity_downgrade", gate_migration::pool_downgrade),
        ("agent_capacity_idle_downgrade", gate_migration::pool_idle_downgrade),
        ("gate_capacity_ended_look", gate_migration::ended_look_and_gate),
        ("agent_capacity_crash_rows", |s| gate_migration::pool_crash(s, "RowsWritten")),
        ("agent_capacity_crash_effects", |s| gate_migration::pool_crash(s, "EffectsWritten")),
        ("agent_capacity_crash_before_commit", |s| gate_migration::pool_crash(s, "BeforeCommit")),
        ("agent_capacity_crash_after_commit", |s| gate_migration::pool_crash(s, "AfterCommit")),
        ("gate_capacity_reopen", gate_migration::reopen),
        ("gate_capacity_downgrade", gate_migration::downgrade),
        (
            "gate_capacity_idle_downgrade",
            gate_migration::idle_downgrade,
        ),
        ("gate_capacity_crash_rows", |s| {
            gate_migration::crash(s, "RowsWritten")
        }),
        ("gate_capacity_crash_effects", |s| {
            gate_migration::crash(s, "EffectsWritten")
        }),
        ("gate_capacity_crash_before_commit", |s| {
            gate_migration::crash(s, "BeforeCommit")
        }),
        ("gate_capacity_crash_after_commit", |s| {
            gate_migration::crash(s, "AfterCommit")
        }),
        ("write_outage_goes_idle", write_outage_goes_idle),
        ("labelled_issue_closes", labelled_issue_closes),
        (
            "restart_without_duplicate_deliveries",
            restart_without_duplicate_deliveries,
        ),
        ("sent_but_absent", sent_but_absent),
        ("acknowledged_operation_look", acknowledged_operation_look),
        (
            "retry_uses_the_kernels_new_key",
            retry_uses_the_kernels_new_key,
        ),
        ("adapter_errors_are_isolated", adapter_errors_are_isolated),
        ("lost_response_keeps_the_key", lost_response_keeps_the_key),
        ("inbox_arrival_order", inbox_arrival_order),
        ("watches_and_pending", watches_and_pending),
        ("reconciled_failure", reconciled_failure),
        (
            "upgrade_with_inflight_effects",
            upgrade_with_inflight_effects,
        ),
        ("cancelled_absence", cancelled_absence),
        ("stale_epoch_effects", stale_epoch_effects),
        ("invalid_footprints", invalid_footprints),
        ("status_is_read_only", status_is_read_only),
        ("status_cli_incident_commands", |settings| {
            status_incident_commands(settings, false, false)
        }),
        ("status_page_incident_commands", |settings| {
            status_incident_commands(settings, true, false)
        }),
        ("status_cli_unposted_incident_commands", |settings| {
            status_incident_commands(settings, false, true)
        }),
        ("status_page_unposted_incident_commands", |settings| {
            status_incident_commands(settings, true, true)
        }),
        ("config_permissions_and_pool", config_permissions_and_pool),
        (
            "fix_round_preserves_uncommitted_work",
            fix_round_preserves_uncommitted_work,
        ),
        ("mixed_registry", mixed_registry),
        ("mixed_registry_conflict", mixed_registry_conflict),
        (
            "merged_discards_rebase_context",
            merged_discards_rebase_context,
        ),
        (
            "stopped_discards_rebase_context",
            stopped_discards_rebase_context,
        ),
        (
            "publication_main_advance_clean",
            publication_main_advance_clean,
        ),
        (
            "publication_main_advance_conflict",
            publication_main_advance_conflict,
        ),
    ];
    let selected = args.iter().find_map(|arg| arg.strip_prefix("--case="));
    for (name, test) in cases {
        if selected.is_some_and(|selected| selected != *name) {
            continue;
        }
        test(&database.create(name));
        println!("test {name} ... ok");
    }
    if let Some(selected) = selected {
        assert!(cases.iter().any(|(name, _)| *name == selected));
        database.stop();
        return;
    }
    status_requires_initialized_database(&database.create("status_fresh"));
    status_page_and_command(&database.create("status"), &mut database);
    println!("test status_page_and_command ... ok");
    database.stop();
    println!("{} controller scenarios passed; no skips", cases.len() + 2);
}

fn config_file(settings: &tl3_config::Store, root: &Path) -> PathBuf {
    let config = include_str!("../../config/example.toml")
        .replace("port = 5432", &format!("port = {}", settings.port))
        .replace(
            "database = \"tl3\"",
            &format!("database = {:?}", settings.database),
        )
        .replace("user = \"tl3\"", "user = \"postgres\"")
        .replace(
            "connect_timeout_secs = 10",
            &format!(
                "password_file = {:?}\nconnect_timeout_secs = 10",
                settings.password_file.as_ref().unwrap()
            ),
        );
    let config = config
        .replace(
            "/home/rob/.config/tl3/github-app.json",
            &root.join("github-fixture").display().to_string(),
        )
        .replace(
            "/run/tl3/model-key",
            &root.join("model-fixture").display().to_string(),
        );
    for name in ["github-fixture", "model-fixture"] {
        let path = root.join(name);
        fs::write(&path, "disposable fixture").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    let path = root.join("config.toml");
    fs::write(&path, config).unwrap();
    path
}
fn cli_status(config: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_tl3"))
        .args(["status", "--config"])
        .arg(config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn labelled_issue_closes(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let records = root.path().join("upstreams");
    let config = config_file(settings, root.path());
    let mut controller = open(settings, &records);
    let mut unauthorized = labelled();
    unauthorized.actor = 1;
    assert!(!controller.intake(unauthorized).unwrap());
    let mut unserved = labelled();
    unserved.repo = "other".into();
    assert!(!controller.intake(unserved).unwrap());
    let mut wrong_installation = labelled();
    wrong_installation.installation = 2;
    assert!(!controller.intake(wrong_installation).unwrap());
    controller.intake(labelled()).unwrap();
    controller.intake(labelled()).unwrap();
    let mut repeated = labelled();
    repeated.delivery = "another-delivery".into();
    controller.intake(repeated).unwrap();
    assert_eq!(controller.world().unwrap().nodes.len(), 1);
    assert!(
        Controller::open(
            Store::open(settings).unwrap(),
            Registry::default(),
            bootstrap()
        )
        .is_err(),
        "a second daemon must not dispatch concurrently"
    );
    let initial = cli_status(&config);
    assert!(
        initial.contains("Understanding the request\trequest reader to start\t0"),
        "{initial}"
    );
    let mut seen = BTreeSet::new();
    for iteration in 0..200 {
        let world = controller.world().unwrap();
        assert!(world.valid() && world.is_settled());
        seen.insert(world.nodes[0].phase.name());
        let mut reader = Reader::open(settings).unwrap();
        let rendered = status::render_text(&status::load(&mut reader).unwrap());
        assert!(rendered.contains(&format!(
            "\t{}\tnone recorded\tnone recorded\n",
            world.nodes[0].credit.balance()
        )));
        if !controller.drive_once().unwrap() {
            break;
        }
        assert!(iteration < 199, "simulation failed to settle");
    }
    for phase in [
        "Ingesting",
        "Auditing",
        "Planning",
        "Making",
        "Checking",
        "Reviewing",
        "Publishing",
        "Queued",
        "Stopping",
        "Closed",
    ] {
        assert!(seen.contains(phase), "missing {phase}: {seen:?}");
    }
    assert_finished(&mut controller, &records, settings);
    let final_status = cli_status(&config);
    assert!(
        final_status
            .contains("RobTand/tl3-sandbox#42: Fix the sandbox build\tClosed: merged\tnone\t5"),
        "{final_status}"
    );
    let snapshot = controller.snapshot().unwrap();
    assert!(
        !controller.world().unwrap().file_sets().is_empty(),
        "large footprints must survive snapshot persistence"
    );
    for kind in [
        tl3_store::InternKind::Repo,
        tl3_store::InternKind::Commit,
        tl3_store::InternKind::Path,
        tl3_store::InternKind::Issue,
        tl3_store::InternKind::PullRequest,
    ] {
        assert!(snapshot.interned.iter().any(|entry| entry.kind == kind));
    }
    let before = snapshot.clone();
    let events = Store::open(settings)
        .unwrap()
        .events_after(tl3_store::Revision::ZERO, 1000)
        .unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.receipt.result.get("kernel_event").is_some())
    );
    drop(controller);
    controller_lock_barrier(settings);
    let mut restored = open(settings, &records);
    assert_eq!(restored.snapshot().unwrap(), before);
    assert!(!restored.drive_once().unwrap());
}

fn assert_finished(controller: &mut Controller, root: &Path, settings: &tl3_config::Store) {
    let world = controller.world().unwrap();
    assert_eq!(world.nodes[0].phase, Phase::Closed(Outcome::Merged));
    assert_eq!(world.nodes[0].credit.balance(), 5);
    assert_eq!(world.nodes[0].head, world.nodes[0].approved);
    assert_eq!(world.nodes[0].approved, world.nodes[0].merged);
    assert!(!world.nodes[0].worktree && !world.nodes[0].pr_open);
    let snapshot = controller.snapshot().unwrap();
    assert!(snapshot.pending_effects.is_empty());
    assert_eq!(
        snapshot.credit.iter().map(|c| c.entry.amount).sum::<i64>(),
        5
    );
    let counts = adapters::delivery_counts(root).unwrap();
    let effects = Reader::open(settings).unwrap().effects().unwrap();
    let expected: BTreeSet<_> = effects
        .iter()
        .filter(|e| e.state != EffectState::Cancelled)
        .map(|e| e.key.clone())
        .collect();
    assert_eq!(
        counts.keys().cloned().collect::<BTreeSet<_>>(),
        expected,
        "missing or unexpected delivery"
    );
    assert!(counts.len() > 10);
    assert!(
        counts.values().all(|n| *n == 1),
        "duplicate deliveries: {counts:?}"
    );
    let mut upstreams = BTreeSet::new();
    for key in counts.keys() {
        let effect = controller_effect(controller, key);
        assert_eq!(effect.state, EffectState::Confirmed);
        let delivery: tl3_controller::executor::Delivery =
            serde_json::from_value(effect.payload).unwrap();
        upstreams.insert(tl3_controller::executor::upstream(delivery.output));
    }
    assert_eq!(
        upstreams,
        BTreeSet::from([
            Upstream::GitHub,
            Upstream::Mergify,
            Upstream::PrismaBuild,
            Upstream::Workers,
            Upstream::Worktrees
        ])
    );
}

fn controller_effect(controller: &mut Controller, key: &str) -> tl3_store::Effect {
    controller.effect(key).unwrap().unwrap()
}

fn restart_without_duplicate_deliveries(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config_file(settings, root.path());
    let records = root.path().join("upstreams");
    kill_at(&config, &records, "committed");
    let snapshot = Store::open(settings).unwrap().load().unwrap();
    assert_eq!(snapshot.pending_effects[0].state, EffectState::Intended);
    assert!(adapters::delivery_counts(&records).unwrap().is_empty());
    let snapshot = kill_at(&config, &records, "dispatched");
    assert_eq!(snapshot.pending_effects[0].state, EffectState::Sent);
    assert_eq!(
        adapters::delivery_counts(&records)
            .unwrap()
            .values()
            .sum::<u64>(),
        1
    );
    for boundary in ["plan", "pr", "enqueue"] {
        kill_at(&config, &records, boundary);
    }
    let mut controller = open(settings, &records);
    for iteration in 0..200 {
        if !controller.drive_once().unwrap() {
            break;
        }
        assert!(iteration < 199);
    }
    assert_finished(&mut controller, &records, settings);
    let events = Store::open(settings)
        .unwrap()
        .events_after(tl3_store::Revision::ZERO, 1000)
        .unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.payload.get("to") == Some(&serde_json::json!("Uncertain")))
    );
    drop(controller);
    controller_lock_barrier(settings);
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_tl3d"))
        .args(["--config"])
        .arg(&config)
        .arg("--simulate")
        .arg(&records)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let input = serde_json::json!({"LabelledIssue": labelled()});
    writeln!(daemon.stdin.take().unwrap(), "{input}").unwrap();
    let output = daemon.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_finished(&mut open(settings, &records), &records, settings);
    assert!(cli_status(&config).contains("Closed: merged\tnone\t5"));
}

fn kill_at(config: &Path, records: &Path, boundary: &str) -> Snapshot {
    let mut child = Command::new(env::current_exe().unwrap())
        .arg("--child")
        .arg(config)
        .arg(records)
        .arg(boundary)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "READY");
    let settings = tl3_config::Config::load(config).unwrap().store;
    let mut reader = Reader::open(&settings).unwrap();
    let before = reader.load().unwrap();
    let before_effects = reader.effects().unwrap();
    let before_events = reader
        .events_after(tl3_store::Revision::ZERO, u32::MAX)
        .unwrap();
    if boundary == "plan" {
        assert!(
            before
                .rows
                .iter()
                .any(|r| r.table == "file_sets" && r.value.as_array().unwrap().len() == 80)
        );
    }
    if boundary == "pr" {
        assert!(
            before
                .rows
                .iter()
                .any(|r| r.table == "pull_requests" && r.value["number"] == 100)
        );
    }
    if boundary == "enqueue" {
        assert!(
            before
                .pending_effects
                .iter()
                .any(|e| e.payload["output"]["Effect"]["kind"] == "Enqueue"
                    && e.state == EffectState::Sent)
        );
    }
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success());
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
    }
    controller_lock_barrier(&settings);
    let after = Reader::open(&settings).unwrap().load().unwrap();
    assert_eq!(after, before, "SIGKILL must preserve every stored row");
    assert_eq!(
        reader.effects().unwrap(),
        before_effects,
        "terminal effects must survive too"
    );
    assert_eq!(
        reader
            .events_after(tl3_store::Revision::ZERO, u32::MAX)
            .unwrap(),
        before_events
    );
    let mut recovered = open(&settings, records);
    assert_eq!(
        recovered.snapshot().unwrap(),
        before,
        "opening must never replay or rewrite the snapshot"
    );
    if before
        .pending_effects
        .first()
        .is_some_and(|e| e.state == EffectState::Sent)
    {
        assert!(recovered.drive_once().unwrap());
        let mut expected = before.clone();
        let actual = recovered.snapshot().unwrap();
        expected.revision = actual.revision;
        expected.pending_effects[0].state = EffectState::Uncertain;
        expected.pending_effects[0].updated_revision = actual.revision;
        assert_eq!(
            actual, expected,
            "recovery may only mark this Sent effect Uncertain"
        );
        let mut expected_effects = before_effects;
        let changed = expected_effects
            .iter_mut()
            .find(|e| e.key == expected.pending_effects[0].key)
            .unwrap();
        changed.state = EffectState::Uncertain;
        changed.updated_revision = actual.revision;
        assert_eq!(reader.effects().unwrap(), expected_effects);
        let mut events = reader
            .events_after(tl3_store::Revision::ZERO, u32::MAX)
            .unwrap();
        let transition = events.pop().unwrap();
        assert_eq!(events, before_events);
        assert_eq!(transition.payload["from"], "Sent");
        assert_eq!(transition.payload["to"], "Uncertain");
        assert_eq!(transition.receipt.revision, actual.revision);
    }
    // Return the parked snapshot, leaving recovery's real transition persisted.
    before
}
fn child(args: &[String]) {
    let config = tl3_config::Config::load(&args[2]).unwrap();
    let mut controller = open(&config.store, Path::new(&args[3]));
    if args[4] == "committed" {
        controller.intake(labelled()).unwrap();
        park();
    }
    loop {
        let key = args[4].as_str();
        let snapshot = controller.snapshot().unwrap();
        if (key == "plan" && snapshot.rows.iter().any(|r| r.table == "file_sets"))
            || (key == "pr" && snapshot.rows.iter().any(|r| r.table == "pull_requests"))
        {
            park();
        }
        assert!(
            controller
                .drive_once_with(|boundary, effect_key| {
                    if (key == "picked_up" && boundary == Boundary::BeforeDispatch)
                        || (key == "dispatched" && boundary == Boundary::AfterDispatch)
                        || (key == "enqueue"
                            && boundary == Boundary::AfterDispatch
                            && snapshot.pending_effects.iter().any(|e| {
                                e.key == effect_key
                                    && e.payload["output"]["Effect"]["kind"] == "Enqueue"
                            }))
                    {
                        park();
                    }
                })
                .unwrap()
        );
    }
}
fn park() {
    println!("READY");
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}

fn drain(controller: &mut Controller) {
    for iteration in 0..300 {
        if !controller.drive_once().unwrap() {
            return;
        }
        assert!(iteration < 299, "controller failed to settle");
    }
}

fn sent_but_absent(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config_file(settings, root.path());
    let records = root.path().join("upstreams");
    kill_at(&config, &records, "committed");
    let parked = kill_at(&config, &records, "picked_up");
    assert!(adapters::delivery_counts(&records).unwrap().is_empty());
    assert_eq!(parked.pending_effects[0].state, EffectState::Sent);
    let mut controller = open(settings, &records);
    drain(&mut controller);
    assert_finished(&mut controller, &records, settings);
    let events = Store::open(settings)
        .unwrap()
        .events_after(tl3_store::Revision::ZERO, 1000)
        .unwrap();
    assert!(
        !events
            .iter()
            .any(|event| event.receipt.result["kernel_event"]["Reply"]["reply"] == "RunFailed"),
        "absence of an unacknowledged start must retry delivery without failing the run"
    );
}

struct WriteDown {
    calls: std::rc::Rc<std::cell::Cell<u64>>,
}
impl Executor for WriteDown {
    fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
        self.calls.set(self.calls.get() + 1);
        Err(tl3_controller::Error::Adapter(Box::new(
            std::io::Error::other("fixture write outage"),
        )))
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(Reconciliation::Absent)
    }
}
fn write_outage_goes_idle(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Workers,
        WriteDown {
            calls: calls.clone(),
        },
    );
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    controller.intake(labelled()).unwrap();
    let before = controller.snapshot().unwrap().revision.get();
    let mut progressed = 0;
    for _ in 0..60 {
        progressed += u64::from(controller.drive_once().unwrap());
    }
    let snapshot = controller.snapshot().unwrap();
    let revisions = snapshot.revision.get() - before;
    println!(
        "write outage: drive_once_true={progressed}/60 revisions_added={revisions} attempts={} deliveries={:?}",
        calls.get(),
        adapters::delivery_counts(root.path()).unwrap()
    );
    assert_eq!(
        progressed, 2,
        "write outage must go idle after Sent and Uncertain"
    );
    assert_eq!(revisions, 2);
    assert_eq!(calls.get(), 1);
    assert!(adapters::delivery_counts(root.path()).unwrap().is_empty());
    assert!(!controller.drive_once().unwrap());
    assert_eq!(controller.snapshot().unwrap(), snapshot);

    controller
        .observe(
            "new-input",
            Observation::PullRequest {
                node: NodeId(0),
                number: 100,
            },
        )
        .unwrap();
    drain(&mut controller);
    assert_eq!(calls.get(), 1, "GitHub news cannot retry a worker write");
    let mut next = labelled();
    next.delivery = "next-input".into();
    controller.intake(next).unwrap();
    drain(&mut controller);
    assert_eq!(calls.get(), 1, "intake cannot retry a worker write");
    controller.retry_held_effects(Upstream::Workers);
    drain(&mut controller);
    assert_eq!(calls.get(), 2, "a worker look retries exactly once");
    drop(controller);
    controller_lock_barrier(settings);
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Workers,
        WriteDown {
            calls: calls.clone(),
        },
    );
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    drain(&mut controller);
    assert_eq!(calls.get(), 3, "a restart retries exactly once");
}

struct DeferredWorker(adapters::workers::Fake);
impl tl3_controller::executor::Executor for DeferredWorker {
    fn execute(
        &mut self,
        delivery: &tl3_controller::executor::Delivery,
    ) -> tl3_controller::Result<tl3_controller::executor::Completion> {
        let mut completion = tl3_controller::executor::Executor::execute(&mut self.0, delivery)?;
        completion.observations.clear();
        Ok(completion)
    }
    fn reconcile(
        &mut self,
        delivery: &tl3_controller::executor::Delivery,
    ) -> tl3_controller::Result<tl3_controller::executor::Reconciliation> {
        tl3_controller::executor::Executor::reconcile(&mut self.0, delivery)
    }
}
fn acknowledged_operation_look(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Workers,
        DeferredWorker(adapters::workers::Fake::new(root.path())),
    );
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    controller.intake(labelled()).unwrap();
    controller.drive_once().unwrap();
    assert!(controller.snapshot().unwrap().pending_effects.is_empty());
    assert!(controller.world().unwrap().nodes[0].run.is_some());
    drop(controller);
    controller_lock_barrier(settings);
    let mut controller = open(settings, root.path());
    drain(&mut controller);
    assert_finished(&mut controller, root.path(), settings);
}

struct RetryWorker {
    fake: adapters::workers::Fake,
    calls: std::rc::Rc<std::cell::RefCell<std::collections::BTreeMap<String, u64>>>,
    first: bool,
}
impl tl3_controller::executor::Executor for RetryWorker {
    fn execute(
        &mut self,
        delivery: &tl3_controller::executor::Delivery,
    ) -> tl3_controller::Result<tl3_controller::executor::Completion> {
        *self
            .calls
            .borrow_mut()
            .entry(delivery.key.clone())
            .or_default() += 1;
        if std::mem::take(&mut self.first) {
            return Ok(tl3_controller::executor::Completion {
                failure_class: None,
                progress: None,
                ack: tl3_kernel::Ack::Retryable,
                observations: vec![],
            });
        }
        tl3_controller::executor::Executor::execute(&mut self.fake, delivery)
    }
    fn reconcile(
        &mut self,
        delivery: &tl3_controller::executor::Delivery,
    ) -> tl3_controller::Result<tl3_controller::executor::Reconciliation> {
        tl3_controller::executor::Executor::reconcile(&mut self.fake, delivery)
    }
}
fn retry_uses_the_kernels_new_key(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let mut registry = adapters::simulation(root.path()).unwrap();
    let calls = std::rc::Rc::new(std::cell::RefCell::new(std::collections::BTreeMap::new()));
    registry.register(
        Upstream::Workers,
        RetryWorker {
            fake: adapters::workers::Fake::new(root.path()),
            calls: calls.clone(),
            first: true,
        },
    );
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    controller.intake(labelled()).unwrap();
    drain(&mut controller);
    assert_finished(&mut controller, root.path(), settings);
    assert_eq!(
        controller.effect("effect:0:0").unwrap().unwrap().state,
        EffectState::Cancelled
    );
    assert!(calls.borrow().values().all(|count| *count == 1));
}

fn controller_lock_barrier(settings: &tl3_config::Store) {
    // Client drop and SIGKILL can precede Postgres releasing the session lock.
    let mut client = raw(settings);
    client
        .query_one("SELECT pg_advisory_lock($1)", &[&0x544c33434f4e5452_i64])
        .unwrap();
    client
        .query_one("SELECT pg_advisory_unlock($1)", &[&0x544c33434f4e5452_i64])
        .unwrap();
}

struct StatusServer(Child);
impl Drop for StatusServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn http(address: std::net::SocketAddr, method: &str, path: &str) -> (String, String) {
    try_http(address, method, path).unwrap()
}

fn try_http(
    address: std::net::SocketAddr,
    method: &str,
    path: &str,
) -> std::io::Result<(String, String)> {
    let mut connection = TcpStream::connect(address)?;
    connection.set_read_timeout(Some(Duration::from_secs(10)))?;
    write!(
        connection,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    )?;
    let mut response = String::new();
    connection.read_to_string(&mut response)?;
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| std::io::Error::other("incomplete HTTP response"))?;
    Ok((headers.to_ascii_lowercase(), body.into()))
}

fn status_incident_commands(settings: &tl3_config::Store, page: bool, unposted: bool) {
    let root = tempfile::tempdir().unwrap();
    let mut controller = open(settings, root.path());
    controller.intake(labelled()).unwrap();
    let mut store = Store::open(settings).unwrap();
    let before = store.load().unwrap();
    let mut change = Commit::new("status-incidents", json!({}), &before);
    let suffix = if unposted {
        " The incident post failed, with class message_rejected."
    } else {
        ""
    };
    for (id, summary, open) in [
        ("3283.0", "The request needs Rob's help.", true),
        ("3284.0", "A second operation needs attention.", true),
        ("3285.0", "This incident is already closed.", false),
    ] {
        change.rows.push(RowChange::Put {
            table: "incidents".into(),
            id: id.into(),
            value: json!({
                "id": id, "node": 0, "open": open, "needs_rob": true,
                "blocking": false, "summary": format!("{summary}{suffix}"),
                "signature": {"upstream": "GitHub", "class": "message_rejected"},
                "post_key": format!("incident:{id}")
            }),
        });
    }
    store.commit(&change).unwrap();
    let before = controller.snapshot().unwrap();
    let config = config_file(settings, root.path());
    let mut server = None;
    let output = if page {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let contents = fs::read_to_string(&config)
            .unwrap()
            .replace("127.0.0.1:8033", &address.to_string());
        fs::write(&config, contents).unwrap();
        drop(listener);
        server = Some(StatusServer(
            Command::new(env!("CARGO_BIN_EXE_tl3"))
                .args(["status-serve", "--config"])
                .arg(&config)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ));
        let mut ready = String::new();
        BufReader::new(server.as_mut().unwrap().0.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert!(ready.contains(&address.to_string()), "{ready}");
        let (headers, body) = http(address, "GET", "/");
        assert!(headers.starts_with("http/1.1 200"), "{headers}");
        body
    } else {
        cli_status(&config)
    };
    for (id, summary) in [
        ("3283.0", "The request needs Rob's help."),
        ("3284.0", "A second operation needs attention."),
    ] {
        assert!(output.contains(&format!("clear incident {id}")), "{output}");
        let escaped = if page { "Rob&#39;s" } else { "Rob's" };
        assert!(
            output.contains(&summary.replace("Rob's", escaped)),
            "{output}"
        );
    }
    assert!(!output.contains("3285.0"), "{output}");
    assert!(
        !output.contains("This incident is already closed."),
        "{output}"
    );
    if unposted {
        assert!(output.contains(suffix), "{output}");
    }
    assert_eq!(
        controller.snapshot().unwrap(),
        before,
        "status must write nothing"
    );
    drop(server);
}

fn status_page_and_command(settings: &tl3_config::Store, database: &mut Database) {
    let root = tempfile::tempdir().unwrap();
    let records = root.path().join("upstreams");
    let mut controller = open(settings, &records);
    assert!(
        status::load(&mut Reader::open(settings).unwrap())
            .unwrap()
            .requests
            .is_empty()
    );
    let mut issue = labelled();
    issue.title = "Fix <script>alert('oops')</script> & spacing\t\n".into();
    controller.intake(issue).unwrap();
    for iteration in 0..100 {
        if controller.world().unwrap().nodes[0].phase == Phase::Making {
            break;
        }
        assert!(controller.drive_once().unwrap());
        assert!(iteration < 99);
    }
    let before = controller.snapshot().unwrap();
    let mut reader = Reader::open(settings).unwrap();
    let model = status::load(&mut reader).unwrap();
    assert_eq!(model.requests.len(), 1);
    let row = &model.requests[0];
    assert_eq!(
        row.issue,
        "RobTand/tl3-sandbox#42: Fix <script>alert('oops')</script> & spacing"
    );
    assert_eq!(row.phase, "Making the change");
    assert_eq!(row.waiting_on, "preparing the change files");
    assert_eq!(row.credit, 4);
    assert_eq!(row.details, status::RequestDetails::default());
    let expected_text = concat!(
        "Request\tPhase\tWaiting on\tCredit\tLast progress\tOpen incidents\n",
        "RobTand/tl3-sandbox#42: Fix <script>alert('oops')</script> & spacing\t",
        "Making the change\tpreparing the change files\t4\tnone recorded\tnone recorded\n"
    );
    assert_eq!(status::render_text(&model), expected_text);
    let expected_html = status::render_html(&model);
    assert!(expected_html.contains("<td>RobTand/tl3-sandbox#42: Fix &lt;script&gt;alert(&#39;oops&#39;)&lt;/script&gt; &amp; spacing</td>"));
    assert!(expected_html.contains("<td>Making the change</td><td>preparing the change files</td><td>4</td><td>none recorded</td><td>none recorded</td>"));
    assert!(!expected_html.contains("<script>"));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let config = config_file(settings, root.path());
    let contents = fs::read_to_string(&config)
        .unwrap()
        .replace("127.0.0.1:8033", &address.to_string());
    fs::write(&config, contents).unwrap();
    drop(listener);
    let mut server = StatusServer(
        Command::new(env!("CARGO_BIN_EXE_tl3"))
            .args(["status-serve", "--config"])
            .arg(&config)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut ready = String::new();
    BufReader::new(server.0.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(
        ready.contains(&address.to_string()),
        "server failed to start: {ready}"
    );
    assert_eq!(cli_status(&config), expected_text);
    let (headers, body) = http(address, "GET", "/");
    assert!(headers.starts_with("http/1.1 200"), "{headers}");
    assert!(!headers.contains("www-authenticate") && !headers.contains("set-cookie"));
    assert!(headers.contains("cache-control: no-store"));
    assert!(headers.contains("connection: close"));
    assert!(headers.contains("x-content-type-options: nosniff"));
    assert!(headers.contains("content-security-policy: default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'"));
    assert_eq!(body, expected_html);
    let (headers, body) = http(address, "HEAD", "/");
    assert!(headers.starts_with("http/1.1 200"));
    assert!(headers.contains(&format!("content-length: {}", expected_html.len())));
    assert!(body.is_empty());
    let mut connection = TcpStream::connect(address).unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    connection
        .write_all(b"POST / HTTP/1.1\r\nHost: first\r\nHost: second\r\n\r\n")
        .unwrap();
    let mut reply = String::new();
    connection.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 405"), "{reply}");
    assert!(reply.contains("Allow: GET, HEAD\r\n"), "{reply}");
    for method in ["POST", "PUT", "DELETE", "OPTIONS"] {
        let (headers, _) = http(address, method, "/");
        assert!(headers.starts_with("http/1.1 405"), "{method}: {headers}");
        assert!(headers.contains("allow: get, head"));
    }
    assert!(
        http(address, "GET", "/missing")
            .0
            .starts_with("http/1.1 404")
    );
    assert_eq!(
        controller.snapshot().unwrap(),
        before,
        "status must write nothing"
    );
    drain(&mut controller);
    let final_model = status::load(&mut reader).unwrap();
    assert_ne!(final_model, model);
    assert_eq!(
        http(address, "GET", "/").1,
        status::render_html(&final_model)
    );
    assert_eq!(cli_status(&config), status::render_text(&final_model));
    status_socket_limits(address);
    let mut large = labelled();
    large.delivery = "large-status-delivery".into();
    large.number = 43;
    large.title = "x".repeat(256 * 1024);
    controller.intake(large).unwrap();
    let slow: Vec<_> = (0..4)
        .map(|_| {
            let mut stream = small_receive_window(address);
            stream
                .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .unwrap();
            stream
        })
        .collect();
    // Small receive windows fill the sockets without expensive page rendering.
    // The fifth request queues behind four replies larger than the TCP windows.
    // Keep those clients open without reading while a normal client is served.
    let (headers, body) = http(address, "HEAD", "/");
    assert!(headers.starts_with("http/1.1 200"));
    assert!(body.is_empty());
    std::thread::sleep(Duration::from_secs(3));
    for mut stream in slow {
        socket2::SockRef::from(&stream)
            .set_recv_buffer_size(1024 * 1024)
            .unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut reply = Vec::new();
        let result = stream.read_to_end(&mut reply);
        assert!(
            result.is_ok() || result.unwrap_err().kind() == std::io::ErrorKind::ConnectionReset
        );
        let header_end = reply.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let headers = std::str::from_utf8(&reply[..header_end]).unwrap();
        let length: usize = headers
            .split("Content-Length: ")
            .nth(1)
            .unwrap()
            .split("\r\n")
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            reply.len() - header_end < length,
            "stuck writer must time out before sending the full body"
        );
    }
    let final_model = status::load(&mut reader).unwrap();
    drop(controller);
    database.restart();
    assert_eq!(
        http(address, "GET", "/").1,
        status::render_html(&final_model)
    );
    database.stop();
    let (headers, body) = http(address, "GET", "/");
    assert!(headers.starts_with("http/1.1 503"));
    assert_eq!(body, "Status is temporarily unavailable.\n");
    let mut logged = String::new();
    BufReader::new(server.0.stderr.take().unwrap())
        .read_line(&mut logged)
        .unwrap();
    assert!(
        logged.contains("status unavailable: Postgres operation failed"),
        "{logged}"
    );
    database.resume();
    assert_eq!(
        http(address, "GET", "/").1,
        status::render_html(&final_model)
    );
}

fn status_requires_initialized_database(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config_file(settings, root.path());
    for command in ["status", "status-serve"] {
        let output = Command::new(env!("CARGO_BIN_EXE_tl3"))
            .args([command, "--config"])
            .arg(&config)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("database not initialized"), "{error}");
        assert!(!error.contains("Postgres operation failed"));
    }
}

fn small_receive_window(address: std::net::SocketAddr) -> TcpStream {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )
    .unwrap();
    // Set before connect so the negotiated TCP window cannot auto-grow.
    socket.set_recv_buffer_size(1024).unwrap();
    socket.connect(&address.into()).unwrap();
    socket.into()
}

fn status_socket_limits(address: std::net::SocketAddr) {
    assert!(http(address, "GET", "/?x=1").0.starts_with("http/1.1 200"));
    assert!(http(address, "HEAD", "/?x=1").0.starts_with("http/1.1 200"));
    assert!(
        http(address, "GET", "/missing?x=1")
            .0
            .starts_with("http/1.1 404")
    );
    let mut split = TcpStream::connect(address).unwrap();
    split
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    split
        .write_all(b"GET /?split=1 HTTP/1.1\r\nHost: local")
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    split.write_all(b"host\r\n\r\n").unwrap();
    let mut response = String::new();
    split.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let mut stalled = TcpStream::connect(address).unwrap();
    stalled.write_all(b"GET / HTTP/1.1\r\nHost:").unwrap();
    assert!(http(address, "GET", "/").0.starts_with("http/1.1 200"));
    stalled
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut response = String::new();
    stalled.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 408"), "{response}");

    let mut oversized = TcpStream::connect(address).unwrap();
    let mut request = b"GET / HTTP/1.1\r\nX-Large: ".to_vec();
    request.resize(8 * 1024, b'x');
    oversized.write_all(&request).unwrap();
    oversized
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut response = String::new();
    oversized.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 431"), "{response}");
    assert!(http(address, "GET", "/").0.starts_with("http/1.1 200"));

    for header in [
        "Content-Length: 1",
        "Transfer-Encoding: chunked",
        "Bad Header: value",
    ] {
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        write!(stream, "GET / HTTP/1.1\r\n{header}\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 400"), "{header}: {response}");
    }

    let blocked: Vec<_> = (0..8)
        .map(|_| TcpStream::connect(address).unwrap())
        .collect();
    let mut refused = TcpStream::connect(address).unwrap();
    refused
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut byte = [0];
    match refused.read(&mut byte) {
        Ok(0) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        result => panic!("ninth connection must be refused immediately: {result:?}"),
    }
    drop(blocked);
    for attempt in 0..20 {
        if try_http(address, "GET", "/")
            .is_ok_and(|(headers, _)| headers.starts_with("http/1.1 200"))
        {
            return;
        }
        assert!(attempt < 19, "status did not recover after clients closed");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn raw(settings: &tl3_config::Store) -> postgres::Client {
    postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap()
}

#[derive(Clone, Copy)]
enum Fault {
    Healthy,
    Unavailable,
    Lost,
    Pending,
    Found(Ack),
}

struct FaultyWorker {
    fake: adapters::workers::Fake,
    fault: std::rc::Rc<std::cell::Cell<Fault>>,
}
impl Executor for FaultyWorker {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        if delivery.node() != NodeId(0) || matches!(self.fault.get(), Fault::Healthy) {
            return self.fake.execute(delivery);
        }
        if matches!(self.fault.get(), Fault::Lost) {
            self.fake.execute(delivery)?;
        }
        Err(tl3_controller::Error::Adapter(Box::new(
            std::io::Error::other("fixture transport error"),
        )))
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        if delivery.node() == NodeId(0) {
            match self.fault.get() {
                Fault::Unavailable => {
                    return Err(tl3_controller::Error::Adapter(Box::new(
                        std::io::Error::other("fixture reconciliation error"),
                    )));
                }
                Fault::Pending => return Ok(Reconciliation::Pending),
                Fault::Found(ack) => {
                    return Ok(Reconciliation::Found(Completion {
                        failure_class: None,
                        progress: None,
                        ack,
                        observations: vec![],
                    }));
                }
                _ => {}
            }
        }
        self.fake.reconcile(delivery)
    }
}
fn faulty(
    settings: &tl3_config::Store,
    root: &Path,
    fault: Fault,
) -> (Controller, std::rc::Rc<std::cell::Cell<Fault>>) {
    let fault = std::rc::Rc::new(std::cell::Cell::new(fault));
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(
        Upstream::Workers,
        FaultyWorker {
            fake: adapters::workers::Fake::new(root),
            fault: fault.clone(),
        },
    );
    let mut bootstrap = bootstrap();
    bootstrap.pool = 2;
    (
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap).unwrap(),
        fault,
    )
}
fn adapter_errors_are_isolated(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let (mut controller, fault) = faulty(settings, root.path(), Fault::Unavailable);
    controller.intake(labelled()).unwrap();
    let mut second = labelled();
    second.delivery = "other-request".into();
    second.number += 1;
    controller.intake(second).unwrap();
    drain(&mut controller);
    assert_eq!(
        controller.world().unwrap().nodes[1].phase,
        Phase::Closed(Outcome::Merged)
    );
    assert_eq!(
        controller_effect(&mut controller, "effect:0:0").state,
        EffectState::Uncertain
    );
    assert!(
        !controller.drive_once().unwrap(),
        "an adapter outage must not spin the daemon"
    );
    fault.set(Fault::Healthy);
    controller.retry_held_effects(Upstream::Workers);
    drain(&mut controller);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)
    );
}
fn lost_response_keeps_the_key(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let (mut controller, fault) = faulty(settings, root.path(), Fault::Lost);
    controller.intake(labelled()).unwrap();
    assert!(controller.drive_once().unwrap());
    assert_eq!(
        controller_effect(&mut controller, "effect:0:0").state,
        EffectState::Sent
    );
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .all(|r| r.table != "inbox")
    );
    assert!(controller.world().unwrap().nodes[0].effect.is_some());
    fault.set(Fault::Pending);
    assert!(controller.drive_once().unwrap());
    let before = controller.snapshot().unwrap();
    assert!(!controller.drive_once().unwrap());
    assert_eq!(
        controller.snapshot().unwrap(),
        before,
        "Pending proves nothing"
    );
    fault.set(Fault::Healthy);
    drain(&mut controller);
    assert_finished(&mut controller, root.path(), settings);
    assert_eq!(
        adapters::delivery_counts(root.path()).unwrap()["effect:0:0"],
        1
    );
}
fn inbox_arrival_order(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let mut controller = open(settings, root.path());
    controller.intake(labelled()).unwrap();
    for (key, id, command) in [("2", 1, Control::Pause), ("12", 2, Control::Resume)] {
        controller
            .observe(
                key,
                Observation::Kernel(Event::Command {
                    node: NodeId(0),
                    id,
                    command,
                }),
            )
            .unwrap();
    }
    let snapshot = controller.snapshot().unwrap();
    let inbox: Vec<_> = snapshot
        .rows
        .iter()
        .filter(|r| r.table == "inbox")
        .collect();
    assert_eq!(
        inbox.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["external:2", "external:12"]
    );
    assert!(inbox[0].arrival < inbox[1].arrival);
    drop(controller);
    controller_lock_barrier(settings);
    let mut controller = open(settings, root.path());
    controller.drive_once().unwrap();
    assert!(controller.world().unwrap().nodes[0].paused);
    controller.drive_once().unwrap();
    assert!(!controller.world().unwrap().nodes[0].paused);
    drain(&mut controller);
    assert_finished(&mut controller, root.path(), settings);
}

struct Deferred {
    inner: Box<dyn Executor>,
    kind: EffectKind,
    ready: std::rc::Rc<std::cell::Cell<bool>>,
}
impl Executor for Deferred {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        let mut completion = self.inner.execute(delivery)?;
        if matches!(delivery.output, Out::Effect { kind, .. } if kind == self.kind) {
            completion.observations.clear();
        }
        Ok(completion)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        if !self.ready.get()
            && matches!(delivery.output, Out::Effect { kind, .. } if kind == self.kind)
        {
            return Ok(Reconciliation::Pending);
        }
        self.inner.reconcile(delivery)
    }
}
fn watches_and_pending(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let ready = std::rc::Rc::new(std::cell::Cell::new(false));
    let mut registry = adapters::simulation(root.path()).unwrap();
    for (upstream, kind, inner) in [
        (
            Upstream::PrismaBuild,
            EffectKind::SubmitChecks,
            Box::new(adapters::prismabuild::Fake::new(root.path())) as Box<dyn Executor>,
        ),
        (
            Upstream::Worktrees,
            EffectKind::StartRebase,
            Box::new(adapters::worktrees::Fake::new(root.path())) as Box<dyn Executor>,
        ),
        (
            Upstream::Mergify,
            EffectKind::Enqueue,
            Box::new(adapters::mergify::Fake::new(root.path())) as Box<dyn Executor>,
        ),
    ] {
        registry.register(
            upstream,
            Deferred {
                inner,
                kind,
                ready: ready.clone(),
            },
        );
    }
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    controller.intake(labelled()).unwrap();
    for kind in [
        EffectKind::SubmitChecks,
        EffectKind::StartRebase,
        EffectKind::Enqueue,
    ] {
        drain(&mut controller);
        let before = controller.snapshot().unwrap();
        let world = controller.world().unwrap();
        let watches: Vec<Delivery> = before
            .rows
            .iter()
            .filter(|r| r.table == "watches")
            .map(|r| serde_json::from_value(r.value.clone()).unwrap())
            .collect();
        assert!(
            watches.iter().any(
                |d| matches!(d.output, Out::Effect {kind: k, ..} if k == kind)
                    && world.nodes[0].operation_pending(kind, d.generation)
            ),
            "missing live {kind:?} watch"
        );
        assert!(!controller.drive_once().unwrap());
        assert_eq!(
            controller.snapshot().unwrap(),
            before,
            "a pending watch changes no state"
        );
        ready.set(true);
        assert!(
            controller.drive_once().unwrap(),
            "look must recover {kind:?}"
        );
        assert!(
            controller.drive_once().unwrap(),
            "consume recovered observation"
        );
        ready.set(false);
        if kind == EffectKind::SubmitChecks {
            // Reach the queue without delivering its merge fact, then require a rebase.
            for _ in 0..100 {
                if controller.world().unwrap().nodes[0].phase == Phase::Queued {
                    break;
                }
                assert!(controller.drive_once().unwrap());
            }
            assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
            controller
                .observe(
                    "conflict",
                    Observation::Kernel(Event::Fact {
                        node: NodeId(0),
                        fact: Fact::Conflict,
                    }),
                )
                .unwrap();
            controller.drive_once().unwrap();
        } else if kind == EffectKind::StartRebase {
            // The rebase requires fresh checks and review. Let those checks complete.
            ready.set(true);
            for _ in 0..100 {
                let world = controller.world().unwrap();
                if world.nodes[0].phase == Phase::Publishing {
                    break;
                }
                assert!(controller.drive_once().unwrap());
            }
            ready.set(false);
        }
    }
    ready.set(true);
    drain(&mut controller);
    // One clean rebase consumes a free recheck and keeps the merge credit once.
    assert_finished(&mut controller, root.path(), settings);
}
fn reconciled_failure(settings: &tl3_config::Store) {
    for ack in [Ack::Retryable, Ack::Permanent] {
        let root = tempfile::tempdir().unwrap();
        // Reset only this disposable schema between the two independent failure outcomes.
        if ack == Ack::Permanent {
            raw(settings)
                .batch_execute("DROP SCHEMA public CASCADE; CREATE SCHEMA public")
                .unwrap();
        }
        let (mut controller, fault) = faulty(settings, root.path(), Fault::Found(ack));
        controller.intake(labelled()).unwrap();
        controller.drive_once().unwrap();
        controller.drive_once().unwrap();
        assert_eq!(
            controller_effect(&mut controller, "effect:0:0").state,
            EffectState::Uncertain
        );
        assert!(controller.drive_once().unwrap());
        let effect = controller_effect(&mut controller, "effect:0:0");
        let world = controller.world().unwrap();
        match ack {
            Ack::Retryable => {
                assert_eq!(effect.state, EffectState::Cancelled);
                assert_eq!(world.nodes[0].effect.unwrap().seq, 1);
                fault.set(Fault::Healthy);
                drain(&mut controller);
                assert_finished(&mut controller, root.path(), settings);
            }
            Ack::Permanent => {
                assert_eq!(effect.state, EffectState::Failed);
                assert_eq!(world.nodes[0].phase, Phase::AwaitingRob);
            }
            _ => unreachable!(),
        }
    }
}

fn upgrade_with_inflight_effects(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let (mut controller, fault) = faulty(settings, root.path(), Fault::Lost);
    controller.intake(labelled()).unwrap();
    controller.drive_once().unwrap();
    let mut store = Store::open(settings).unwrap();
    for state in [EffectState::Sent, EffectState::Uncertain] {
        let before = controller.snapshot().unwrap();
        assert_eq!(before.pending_effects[0].state, state);
        let mut upgrade = Commit::new("upgrade", serde_json::json!({"format": 1}), &before);
        upgrade.next_epoch = Some(Epoch::try_from(1).unwrap());
        assert!(matches!(
            store.commit(&upgrade),
            Err(tl3_store::Error::ActiveDeliveries)
        ));
        assert_eq!(store.load().unwrap(), before);
        if state == EffectState::Sent {
            controller.drive_once().unwrap();
        }
    }
    fault.set(Fault::Healthy);
    controller.drive_once().unwrap(); // Find the original start, including its reply.
    controller.drive_once().unwrap(); // Its reply starts the audit, still Intended.
    let before = controller.snapshot().unwrap();
    assert!(
        before
            .pending_effects
            .iter()
            .all(|e| e.state == EffectState::Intended)
    );
    assert!(!before.pending_effects.is_empty());
    let mut upgrade = Commit::new("upgrade", serde_json::json!({"format": 1}), &before);
    upgrade.next_epoch = Some(Epoch::try_from(1).unwrap());
    store.commit(&upgrade).unwrap();
    let upgraded = store.load().unwrap();
    assert_eq!(upgraded.epoch.get(), 1);
    assert_eq!(upgraded.rows, before.rows);
    assert_eq!(upgraded.credit, before.credit);
    assert_eq!(upgraded.interned, before.interned);
    for (old, new) in before.pending_effects.iter().zip(&upgraded.pending_effects) {
        let mut expected = old.clone();
        expected.epoch = upgraded.epoch;
        expected.updated_revision = upgraded.revision;
        assert_eq!(*new, expected, "upgrade preserves keys and payloads");
    }
    drain(&mut controller);
    assert_finished(&mut controller, root.path(), settings);
    let epochs = adapters::delivery_epochs(root.path()).unwrap();
    assert_eq!(epochs["effect:0:0"], [0]);
    for effect in upgraded.pending_effects {
        assert_eq!(epochs[&effect.key], [1]);
    }
    let old_effect = controller_effect(&mut controller, "effect:0:0");
    let mut stale: Delivery = serde_json::from_value(old_effect.payload).unwrap();
    stale.key = "refused-old-epoch".into();
    let mut restarted_fake = adapters::workers::Fake::new(root.path());
    assert!(
        restarted_fake.execute(&stale).is_err(),
        "fence must survive adapter restart"
    );
    assert!(
        !adapters::delivery_counts(root.path())
            .unwrap()
            .contains_key(&stale.key)
    );
    stale.key = "effect:0:0".into();
    assert!(
        matches!(
            restarted_fake.reconcile(&stale).unwrap(),
            Reconciliation::Found(_)
        ),
        "old epoch records remain readable"
    );
}

fn cancelled_absence(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let (mut controller, fault) = faulty(settings, root.path(), Fault::Pending);
    controller.intake(labelled()).unwrap();
    controller.drive_once().unwrap();
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
    controller.drive_once().unwrap();
    controller.drive_once().unwrap();
    assert_eq!(
        controller_effect(&mut controller, "effect:0:0").state,
        EffectState::Cancelling
    );
    let before = controller.snapshot().unwrap();
    let mut upgrade = Commit::new("upgrade", serde_json::json!(null), &before);
    upgrade.next_epoch = Some(Epoch::try_from(1).unwrap());
    assert!(matches!(
        Store::open(settings).unwrap().commit(&upgrade),
        Err(tl3_store::Error::ActiveDeliveries)
    ));
    assert!(
        !controller.drive_once().unwrap(),
        "Pending cancellation is not absence"
    );
    fault.set(Fault::Healthy);
    controller.drive_once().unwrap();
    assert_eq!(
        controller_effect(&mut controller, "effect:0:0").state,
        EffectState::Cancelled
    );
    let events = Reader::open(settings)
        .unwrap()
        .events_after(before.revision, 10)
        .unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.receipt.result["kernel_event"]["Ack"]["ack"] == "Retryable")
    );
    assert!(
        !adapters::delivery_counts(root.path())
            .unwrap()
            .contains_key("effect:0:0")
    );
    drain(&mut controller);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Stopped)
    );

    let mut next = labelled();
    next.delivery = "stop-before-dispatch".into();
    next.number += 1;
    controller.intake(next).unwrap();
    controller
        .observe(
            "stop-intended",
            Observation::Kernel(Event::Command {
                node: NodeId(1),
                id: 1,
                command: Control::Stop,
            }),
        )
        .unwrap();
    controller.drive_once().unwrap();
    drain(&mut controller);
    assert_eq!(
        controller_effect(&mut controller, "effect:1:0").state,
        EffectState::Cancelled
    );
    assert!(
        !adapters::delivery_counts(root.path())
            .unwrap()
            .contains_key("effect:1:0")
    );
    assert_eq!(
        controller.world().unwrap().nodes[1].phase,
        Phase::Closed(Outcome::Stopped)
    );
}

fn stale_epoch_effects(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(Upstream::Workers, adapters::workers::Fake::new(root.path()));
    let mut boot = bootstrap();
    boot.pool = 3;
    let mut controller = Controller::open(Store::open(settings).unwrap(), registry, boot).unwrap();
    for i in 0..3 {
        let mut issue = labelled();
        issue.delivery = format!("stale-{i}");
        issue.number += i;
        controller.intake(issue).unwrap();
    }
    let mut store = Store::open(settings).unwrap();
    for (key, states) in [
        ("effect:1:0", vec![EffectState::Sent]),
        (
            "effect:2:0",
            vec![EffectState::Sent, EffectState::Uncertain],
        ),
    ] {
        let mut from = EffectState::Intended;
        for to in states {
            let before = store.load().unwrap();
            let mut commit = Commit::new(
                format!("seed:{key}:{to:?}"),
                serde_json::json!(null),
                &before,
            );
            commit.transitions.push(EffectTransition {
                key: key.into(),
                from,
                to,
            });
            store.commit(&commit).unwrap();
            from = to;
        }
    }
    let found = store.effect("effect:2:0").unwrap().unwrap();
    let delivery: Delivery = serde_json::from_value(found.payload).unwrap();
    adapters::workers::Fake::new(root.path())
        .execute(&delivery)
        .unwrap();
    // An inconsistent migration fixture exercises defensive recovery. Normal upgrades
    // cannot do this: upgrade_with_inflight_effects proves the store's drain guard.
    raw(settings)
        .execute("UPDATE tl3_meta SET epoch = 1", &[])
        .unwrap();
    drain(&mut controller);
    assert!(
        controller
            .world()
            .unwrap()
            .nodes
            .iter()
            .all(|n| n.phase == Phase::Closed(Outcome::Merged))
    );
    for key in ["effect:0:0", "effect:1:0"] {
        assert_eq!(
            controller_effect(&mut controller, key).state,
            EffectState::Cancelled
        );
        assert!(
            !adapters::delivery_counts(root.path())
                .unwrap()
                .contains_key(key)
        );
    }
    assert_eq!(
        controller_effect(&mut controller, "effect:2:0").state,
        EffectState::Confirmed
    );
    let epochs = adapters::delivery_epochs(root.path()).unwrap();
    assert_eq!(epochs["effect:2:0"], [0]);
    assert!(
        epochs
            .iter()
            .filter(|(key, _)| key.as_str() != "effect:2:0")
            .all(|(_, epochs)| *epochs == [1])
    );
    let expected: BTreeSet<_> = store
        .effects()
        .unwrap()
        .into_iter()
        .filter(|e| e.state != EffectState::Cancelled)
        .map(|e| e.key)
        .collect();
    assert_eq!(
        adapters::delivery_counts(root.path())
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        expected
    );
}

fn invalid_footprints(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let mut controller = open(settings, root.path());
    let mut store = Store::open(settings).unwrap();
    for (key, files) in [("unsorted", vec![65, 64]), ("duplicate", vec![64, 64])] {
        let before = store.load().unwrap();
        let mut change = Commit::new(key, serde_json::json!(files), &before);
        change.rows.push(RowChange::Put {
            table: "file_sets".into(),
            id: "0".into(),
            value: serde_json::json!(files),
        });
        store.commit(&change).unwrap();
        assert!(matches!(
            controller.world(),
            Err(tl3_controller::Error::Invalid(
                "stored footprint must be sorted and deduplicated"
            ))
        ));
        assert_eq!(
            status::load(&mut Reader::open(settings).unwrap())
                .unwrap_err()
                .to_string(),
            "stored footprint must be sorted and deduplicated"
        );
    }
    drop(controller);
    controller_lock_barrier(settings);
    let error = Controller::open(
        Store::open(settings).unwrap(),
        Registry::default(),
        bootstrap(),
    )
    .err()
    .expect("invalid footprint must reject controller recovery");
    assert_eq!(
        error.to_string(),
        "stored footprint must be sorted and deduplicated"
    );
}

fn status_is_read_only(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config_file(settings, root.path());
    assert!(matches!(
        Reader::open(settings),
        Err(tl3_store::Error::NotInitialized)
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_tl3"))
        .args(["status", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("database not initialized"));
    assert_eq!(
        raw(settings)
            .query_one(
                "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public'",
                &[]
            )
            .unwrap()
            .get::<_, i64>(0),
        0,
        "status must not migrate an empty database"
    );
    let mut writer = Store::open(settings).unwrap();
    assert!(
        status::load(&mut Reader::open(settings).unwrap())
            .unwrap()
            .requests
            .is_empty()
    );
    let mut controller = open(settings, &root.path().join("upstreams"));
    controller.intake(labelled()).unwrap();
    let before = writer.load().unwrap();
    assert!(cli_status(&config).contains("Understanding the request"));
    let mut reader = Reader::open(settings).unwrap();
    assert!(
        reader
            .transaction()
            .unwrap()
            .batch_execute("CREATE TABLE must_not_write (id integer)")
            .is_err()
    );
    assert!(
        reader
            .transaction()
            .unwrap()
            .execute(
                "INSERT INTO tl3_intern (kind, scope, value) VALUES ('Repo', 'must', 'not-write')",
                &[]
            )
            .is_err()
    );
    assert_eq!(writer.load().unwrap(), before);
}

fn config_permissions_and_pool(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config_file(settings, root.path());
    let source = fs::read_to_string(&config)
        .unwrap()
        .replace("agent_pool = 1", "agent_pool = 3");
    fs::write(&config, source).unwrap();
    let loaded = tl3_config::Config::load(&config).unwrap();
    assert_eq!(Bootstrap::from_config(&loaded).unwrap().pool, 3);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let password = settings.password_file.as_ref().unwrap();
        fs::set_permissions(password, fs::Permissions::from_mode(0o644)).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_tl3"))
            .args(["status", "--config"])
            .arg(&config)
            .output()
            .unwrap();
        fs::set_permissions(password, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("store.password_file"));
    }
}

mod worktree_fixture;

struct GitHeadWorker {
    fake: adapters::workers::Fake,
    root: PathBuf,
    conflict: bool,
    heads: BTreeMap<String, String>,
}
impl GitHeadWorker {
    fn actual_head(&mut self, delivery: &Delivery, mut completion: Completion) -> Completion {
        for observation in &mut completion.observations {
            if let Observation::Made { sha, .. } = observation {
                *sha = self
                    .heads
                    .entry(delivery.key.clone())
                    .or_insert_with(|| {
                        let fixture = worktree_fixture::Fixture::open(&self.root);
                        worktree_fixture::commit(
                            &fixture.adapter().checkout(delivery),
                            if self.conflict {
                                "shared.txt"
                            } else {
                                "leaf.txt"
                            },
                            &format!("maker {}\n", delivery.key),
                        )
                    })
                    .clone();
            }
        }
        completion
    }
}
impl Executor for GitHeadWorker {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        let completion = self.fake.execute(delivery)?;
        Ok(self.actual_head(delivery, completion))
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(match self.fake.reconcile(delivery)? {
            Reconciliation::Found(completion) => {
                Reconciliation::Found(self.actual_head(delivery, completion))
            }
            other => other,
        })
    }
}
struct ConflictQueue {
    fake: adapters::mergify::Fake,
    root: PathBuf,
    first: Option<String>,
    conflict: bool,
}
impl ConflictQueue {
    fn result(&self, delivery: &Delivery, mut completion: Completion) -> Completion {
        if self.first.as_deref() == Some(&delivery.key) {
            completion.observations = vec![Observation::Kernel(Event::Fact {
                node: delivery.node(),
                fact: Fact::Conflict,
            })];
        }
        completion
    }
}
impl Executor for ConflictQueue {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        if self.first.is_none() {
            worktree_fixture::Fixture::open(&self.root).advance_main(self.conflict);
            self.first = Some(delivery.key.clone());
        }
        let completion = self.fake.execute(delivery)?;
        Ok(self.result(delivery, completion))
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(match self.fake.reconcile(delivery)? {
            Reconciliation::Found(completion) => {
                Reconciliation::Found(self.result(delivery, completion))
            }
            other => other,
        })
    }
}
fn mixed_registry(settings: &tl3_config::Store) {
    mixed_registry_case(settings, false);
}
fn mixed_registry_conflict(settings: &tl3_config::Store) {
    mixed_registry_case(settings, true);
}

fn merged_discards_rebase_context(settings: &tl3_config::Store) {
    closure_discards_rebase_context(settings, false);
}

fn stopped_discards_rebase_context(settings: &tl3_config::Store) {
    closure_discards_rebase_context(settings, true);
}

fn closure_discards_rebase_context(settings: &tl3_config::Store, stopped: bool) {
    let root = tempfile::tempdir().unwrap();
    let mut controller = open(settings, root.path());
    controller.intake(labelled()).unwrap();
    let phase = if stopped {
        Phase::Making
    } else {
        Phase::Queued
    };
    for _ in 0..100 {
        if controller.world().unwrap().nodes[0].phase == phase {
            break;
        }
        assert!(controller.drive_once().unwrap());
    }
    let world = controller.world().unwrap();
    assert_eq!(world.nodes[0].phase, phase);
    let mut store = Store::open(settings).unwrap();
    let mut saved = Commit::new(
        "rebase-context-before-close",
        json!({}),
        &store.load().unwrap(),
    );
    saved.rows.push(RowChange::Put {
        table: "rebase_conflicts".into(),
        id: "0".into(),
        value: json!({"head":"old", "target":"old", "files":["greeting.py"]}),
    });
    store.commit(&saved).unwrap();
    let event = if stopped {
        Event::Command {
            node: NodeId(0),
            id: 1,
            command: Control::Stop,
        }
    } else {
        Event::Fact {
            node: NodeId(0),
            fact: Fact::PrMerged {
                head: world.nodes[0].head.unwrap(),
                checks_passed: true,
            },
        }
    };
    controller
        .observe("close-with-rebase-context", Observation::Kernel(event))
        .unwrap();
    if !stopped {
        for _ in 0..100 {
            if controller.world().unwrap().nodes[0].merged.is_some() {
                break;
            }
            assert!(controller.drive_once().unwrap());
        }
        assert!(matches!(
            controller.world().unwrap().nodes[0].phase,
            Phase::Stopping(Outcome::Merged)
        ));
        assert!(
            !controller
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "rebase_conflicts"),
            "a merge must discard rebase context before teardown finishes"
        );
    }
    drain(&mut controller);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(if stopped {
            Outcome::Stopped
        } else {
            Outcome::Merged
        })
    );
    assert!(
        !controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "rebase_conflicts"),
        "a closed node must not retain rebase context"
    );
}

fn publication_main_advance_clean(settings: &tl3_config::Store) {
    publication_main_advance(settings, false);
}

fn publication_main_advance_conflict(settings: &tl3_config::Store) {
    publication_main_advance(settings, true);
}

fn publication_main_advance(settings: &tl3_config::Store, conflict: bool) {
    let root = tempfile::tempdir().unwrap();
    let f = worktree_fixture::Fixture::create(root.path());
    let records = root.path().join("fakes");
    let mut registry = adapters::simulation(&records).unwrap();
    registry.register(Upstream::Worktrees, f.adapter());
    registry.register(
        Upstream::Workers,
        GitHeadWorker {
            fake: adapters::workers::Fake::new(&records),
            root: root.path().into(),
            conflict: true,
            heads: BTreeMap::new(),
        },
    );
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    controller.intake(labelled()).unwrap();
    for _ in 0..100 {
        if controller.world().unwrap().nodes[0].phase == Phase::Publishing {
            break;
        }
        assert!(controller.drive_once().unwrap());
    }
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Publishing
    );
    f.advance_main(conflict);
    for _ in 0..200 {
        if !controller.drive_once().unwrap() {
            break;
        }
    }
    let snapshot = controller.snapshot().unwrap();
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged),
        "publication must recover from a main advance without Rob"
    );
    assert!(!snapshot.rows.iter().any(|r| r.table == "incidents"));
}

fn mixed_registry_case(settings: &tl3_config::Store, conflict: bool) {
    use worktree_fixture::{Fixture, git};
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::create(root.path());
    let records = root.path().join("upstreams");
    let mut registry = adapters::simulation(&records).unwrap();
    registry.register(Upstream::Worktrees, fixture.adapter());
    registry.register(
        Upstream::Workers,
        GitHeadWorker {
            fake: adapters::workers::Fake::new(&records),
            root: root.path().into(),
            conflict,
            heads: BTreeMap::new(),
        },
    );
    registry.register(
        Upstream::Mergify,
        ConflictQueue {
            fake: adapters::mergify::Fake::new(&records),
            root: root.path().into(),
            first: None,
            conflict,
        },
    );
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    controller.intake(labelled()).unwrap();
    let prepare = fixture.delivery(EffectKind::PrepareWorktree, 1);
    let checkout = fixture.adapter().checkout(&prepare);
    let branch = fixture.branch(&prepare);
    let mut created = false;
    let mut pushed = false;
    let mut rebased = false;
    let mut original = None;
    let mut needs_changes = false;
    let mut queued_credit = 0;
    let mut queued_attempt_round = (0, 0);
    let mut maker_contents = String::new();
    let mut returned_to_maker = false;
    for _ in 0..300 {
        let world = controller.world().unwrap();
        if checkout.join(".git").is_file() {
            created = true;
        }
        if world.nodes[0].phase == Phase::Queued && !pushed {
            if !conflict {
                let mut store = Store::open(settings).unwrap();
                let mut saved =
                    Commit::new("old-rebase-context", json!({}), &store.load().unwrap());
                saved.rows.push(RowChange::Put {
                    table: "rebase_conflicts".into(),
                    id: "0".into(),
                    value: json!({"head":"old", "target":"old", "files":["shared.txt"]}),
                });
                store.commit(&saved).unwrap();
            }
            original = Some(fixture.head(&prepare));
            queued_credit = world.nodes[0].credit.balance();
            queued_attempt_round = (world.nodes[0].attempt, world.nodes[0].round);
            maker_contents =
                fs::read_to_string(checkout.join(if conflict { "shared.txt" } else { "leaf.txt" }))
                    .unwrap();
            assert_eq!(
                git(&fixture.remote, &["rev-parse", branch.as_str()]),
                original.clone().unwrap()
            );
            assert_eq!(
                git(
                    &fixture.repo,
                    &[
                        "rev-list",
                        "--count",
                        &format!("refs/tl3/bases/{}..{}", branch.as_str(), branch.as_str())
                    ]
                ),
                "1"
            );
            pushed = true;
        }
        if conflict && pushed && world.nodes[0].phase == Phase::Rebasing {
            let snapshot = controller.snapshot().unwrap();
            needs_changes |= snapshot.rows.iter().any(|row| {
                row.table == "inbox"
                    && serde_json::from_value::<Observation>(row.value.clone()).is_ok_and(
                        |observation| matches!(observation, Observation::RebaseConflict { .. }),
                    )
            });
            if needs_changes {
                assert_eq!(Some(fixture.head(&prepare)), original);
                assert!(git(&checkout, &["status", "--porcelain"]).contains("UU shared.txt"));
                assert_eq!(
                    git(&checkout, &["rev-parse", "MERGE_HEAD"]),
                    git(&fixture.remote, &["rev-parse", "main"])
                );
            }
        }
        if needs_changes && world.nodes[0].phase == Phase::Making {
            returned_to_maker = true;
            assert_eq!(
                (world.nodes[0].attempt, world.nodes[0].round),
                queued_attempt_round
            );
            assert_eq!(world.nodes[0].credit.balance(), queued_credit - 1);
        }
        if pushed && world.nodes[0].phase == Phase::Checking && conflict {
            let head = fixture.head(&prepare);
            let main = git(&fixture.remote, &["rev-parse", "main"]);
            assert_eq!(
                git(&fixture.repo, &["merge-base", &head, &main]),
                main,
                "a conflict maker must finish integration onto main"
            );
        }
        if pushed && world.nodes[0].phase == Phase::Checking && !conflict {
            let head = fixture.head(&prepare);
            assert_ne!(Some(head.clone()), original);
            let snapshot = controller.snapshot().unwrap();
            assert!(
                !snapshot
                    .rows
                    .iter()
                    .any(|row| row.table == "rebase_conflicts" && row.id == "0"),
                "a clean rebase must delete obsolete conflict context"
            );
            assert!(
                snapshot
                    .interned
                    .iter()
                    .any(|i| i.kind == tl3_store::InternKind::Commit && i.value == head)
            );
            assert_eq!(
                fs::read_to_string(checkout.join("leaf.txt")).unwrap(),
                maker_contents
            );
            assert_eq!(
                git(
                    &fixture.repo,
                    &[
                        "rev-list",
                        "--count",
                        &format!("refs/tl3/bases/{}..{}", branch.as_str(), branch.as_str())
                    ]
                ),
                "1"
            );
            rebased = true;
        }
        if !controller.drive_once().unwrap() {
            break;
        }
    }
    assert!(
        created
            && pushed
            && if conflict {
                needs_changes && returned_to_maker
            } else {
                rebased
            },
        "real lifecycle incomplete: created={created}, pushed={pushed}, rebased={rebased}, phase={:?}",
        controller.world().unwrap().nodes[0].phase
    );
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)
    );
    assert!(!checkout.exists());
    assert!(
        git(
            &fixture.repo,
            &["for-each-ref", "refs/heads/tl3", "refs/tl3/bases/tl3"]
        )
        .is_empty()
    );
    assert!(git(&fixture.remote, &["for-each-ref", "refs/heads/tl3"]).is_empty());
    let counts = adapters::delivery_counts(&records).unwrap();
    assert!(counts.len() > 10, "other upstreams use persistent fakes");
    assert!(counts.values().all(|count| *count == 1));
}

struct FixRoundWorker {
    fake: adapters::workers::Fake,
    root: PathBuf,
    makes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
impl Executor for FixRoundWorker {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        use std::sync::atomic::Ordering;
        use tl3_kernel::{Reply, Role};
        use worktree_fixture::{Fixture, commit, git};
        let mut completion = self.fake.execute(delivery)?;
        match delivery.output {
            Out::Effect {
                kind: EffectKind::StartRun(Role::Make),
                ..
            } => {
                let checkout = Fixture::open(&self.root).adapter().checkout(delivery);
                let make = self.makes.fetch_add(1, Ordering::Relaxed);
                if make == 1 {
                    assert_eq!(delivery.round, 1);
                    assert_eq!(
                        fs::read_to_string(checkout.join("shared.txt")).unwrap(),
                        "maker wip\n"
                    );
                    assert_eq!(
                        fs::read_to_string(checkout.join("notes.txt")).unwrap(),
                        "maker notes\n"
                    );
                    assert!(!git(&checkout, &["status", "--porcelain"]).is_empty());
                }
                let sha = commit(&checkout, "leaf.txt", &format!("maker round {make}\n"));
                if make == 0 {
                    fs::write(checkout.join("shared.txt"), "maker wip\n").unwrap();
                    fs::write(checkout.join("notes.txt"), "maker notes\n").unwrap();
                }
                completion.observations = vec![Observation::Made {
                    node: delivery.node(),
                    generation: delivery.generation,
                    sha,
                }];
            }
            Out::Effect {
                kind: EffectKind::StartRun(Role::Review),
                ..
            } if delivery.round == 0 => {
                completion.observations = vec![Observation::Kernel(Event::Reply {
                    node: delivery.node(),
                    generation: delivery.generation,
                    reply: Reply::ChangesRequested,
                })];
            }
            _ => {}
        }
        Ok(completion)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.fake.reconcile(delivery)
    }
}

struct FixRoundWorktrees {
    real: adapters::worktrees::Real,
    prepares: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
impl Executor for FixRoundWorktrees {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        use std::sync::atomic::Ordering;
        use worktree_fixture::git;
        if matches!(
            delivery.output,
            Out::Effect {
                kind: EffectKind::PrepareWorktree,
                ..
            }
        ) {
            let count = self.prepares.fetch_add(1, Ordering::Relaxed);
            if count == 1 {
                assert_eq!(delivery.round, 1);
                let checkout = self.real.checkout(delivery);
                let head = git(&checkout, &["rev-parse", "HEAD"]);
                let status = git(
                    &checkout,
                    &["status", "--porcelain", "--untracked-files=all"],
                );
                let completion = self.real.execute(delivery)?;
                assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), head);
                assert_eq!(
                    git(
                        &checkout,
                        &["status", "--porcelain", "--untracked-files=all"]
                    ),
                    status
                );
                assert_eq!(
                    fs::read_to_string(checkout.join("shared.txt")).unwrap(),
                    "maker wip\n"
                );
                assert_eq!(
                    fs::read_to_string(checkout.join("notes.txt")).unwrap(),
                    "maker notes\n"
                );
                return Ok(completion);
            }
        }
        self.real.execute(delivery)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.real.reconcile(delivery)
    }
}

fn fix_round_preserves_uncommitted_work(settings: &tl3_config::Store) {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let root = tempfile::tempdir().unwrap();
    let fixture = worktree_fixture::Fixture::create(root.path());
    let records = root.path().join("upstreams");
    let makes = Arc::new(AtomicUsize::new(0));
    let prepares = Arc::new(AtomicUsize::new(0));
    let mut registry = adapters::simulation(&records).unwrap();
    registry.register(
        Upstream::Worktrees,
        FixRoundWorktrees {
            real: fixture.adapter(),
            prepares: prepares.clone(),
        },
    );
    registry.register(
        Upstream::Workers,
        FixRoundWorker {
            fake: adapters::workers::Fake::new(&records),
            root: root.path().into(),
            makes: makes.clone(),
        },
    );
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    controller.intake(labelled()).unwrap();
    drain(&mut controller);
    assert_eq!(prepares.load(Ordering::Relaxed), 2);
    assert_eq!(makes.load(Ordering::Relaxed), 2);
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)
    );
}
