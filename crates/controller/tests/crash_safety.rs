#[path = "../../store/tests/support/mod.rs"]
mod support;
#[path = "crash/upstreams.rs"]
mod upstreams;

use serde_json::{Value, json};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
    rc::Rc,
    time::{Duration, Instant, SystemTime},
};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository,
    clock::Clock,
    executor::{Delivery, Observation},
};
use tl3_kernel::{Command as Control, Event, NodeId, Out, Outcome, Phase};
use tl3_store::{Commit, EffectState, Epoch, InternKind, Revision, Store};

const POINTS: &[&str] = &[
    "EventWritten",
    "RowsWritten",
    "EffectsWritten",
    "TransitionsWritten",
    "CreditWritten",
    "BeforeCommit",
    "AfterCommit",
    "InternBeforeCommit",
    "InternAfterCommit",
    "BeforeDispatch",
    "AfterDispatch",
    "BeforeReconcile",
    "AfterReconcile",
];
const SCENARIOS: &[&str] = &[
    "merge",
    "rebase",
    "stop-run",
    "stop-checks",
    "stop-pr",
    "stop-intended",
    "stop-sent",
    "stop-absent",
    "retry",
    "permanent",
    "epoch",
];

#[derive(Clone)]
struct FakeClock(Rc<Cell<u64>>);
impl Clock for FakeClock {
    fn now(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(self.0.get())
    }
}

fn bootstrap() -> Bootstrap {
    Bootstrap {
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
fn issue(seed: u64) -> LabelledIssue {
    LabelledIssue {
        delivery: format!("seed-{seed}"),
        owner: "RobTand".into(),
        repo: "tl3-sandbox".into(),
        installation: 1,
        number: 42,
        actor: 24923402,
        label: "tl3".into(),
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
fn probe(point: &str, key: &str) {
    println!("{}", json!({"point": point, "key": key}));
    std::io::stdout().flush().unwrap();
    let mut command = String::new();
    assert_ne!(
        std::io::stdin().read_line(&mut command).unwrap(),
        0,
        "parent disconnected"
    );
    assert_eq!(command.trim(), "continue");
}
fn cut_key(key: &str, event: &Value) -> String {
    if key.starts_with("delivery:") {
        // Recovery can cycle Sent -> Uncertain -> Intended on the same key.
        // A revision-based cut would kill every pickup forever.
        format!(
            "delivery:{}:{}:{}",
            event["effect"], event["from"], event["to"]
        )
    } else {
        key.into()
    }
}

fn hooked_store(settings: &tl3_config::Store, selected: &str) -> Store {
    let mut store = Store::open(settings).unwrap();
    let selected = selected.to_owned();
    store.set_boundary_hook(move |point, key, event| {
        let point = format!("{point:?}");
        if point == selected {
            probe(&point, &cut_key(key, event));
        }
    });
    store
}
fn settings_file(settings: &tl3_config::Store, root: &Path) -> std::path::PathBuf {
    let path = root.join("settings.json");
    fs::write(&path, serde_json::to_vec(&json!({"host":settings.host, "port":settings.port, "database":settings.database, "user":settings.user, "password_file":settings.password_file, "connect_timeout_secs":settings.connect_timeout_secs})).unwrap()).unwrap();
    path
}

fn child(args: &[String]) {
    let settings: tl3_config::Store = serde_json::from_slice(&fs::read(&args[2]).unwrap()).unwrap();
    let root = Path::new(&args[3]);
    let scenario = args[4].as_str();
    let selected = args[5].as_str();
    let seed: u64 = args[6].parse().unwrap();
    let clock = FakeClock(Rc::new(Cell::new(seed % 1_000_000_000)));
    let mut controller = Controller::open_with_clock(
        hooked_store(&settings, selected),
        upstreams::registry(root, scenario, seed),
        bootstrap(),
        clock.clone(),
    )
    .unwrap();
    controller.intake(issue(seed)).unwrap();
    for turn in 0..500 {
        clock.0.set(seed % 1_000_000_000 + turn * 86400);
        assert_eq!(controller.now(), clock.now());
        let world = controller.world().unwrap();
        assert!(world.valid() && world.is_settled());
        let node = &world.nodes[0];
        let snapshot = controller.snapshot().unwrap();
        let stop = match scenario {
            "stop-intended" => node.phase == Phase::Ingesting,
            "stop-sent" | "stop-absent" => {
                (scenario == "stop-absent"
                    || upstreams::load(&upstreams::path(
                        root,
                        tl3_controller::executor::Upstream::Workers,
                    ))
                    .records
                    .contains_key("effect:0:0"))
                    && snapshot
                        .pending_effects
                        .iter()
                        .any(|e| e.key == "effect:0:0" && e.state == EffectState::Sent)
            }
            "stop-run" => {
                node.phase == Phase::Making && node.run.is_some() && node.effect.is_none()
            }
            "stop-checks" => {
                node.phase == Phase::Checking && node.job.is_some() && node.effect.is_none()
            }
            "stop-pr" => node.phase == Phase::Queued,
            _ => false,
        };
        if stop {
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
        }
        if scenario == "epoch" && snapshot.epoch == Epoch::ZERO {
            let mut store = hooked_store(&settings, selected);
            let mut upgrade = Commit::new("upgrade", json!({"format": 1}), &snapshot);
            upgrade.next_epoch = Some(Epoch::try_from(1).unwrap());
            if snapshot.pending_effects.iter().any(|e| {
                matches!(
                    e.state,
                    EffectState::Sent | EffectState::Uncertain | EffectState::Cancelling
                )
            }) {
                assert!(matches!(
                    store.commit(&upgrade),
                    Err(tl3_store::Error::ActiveDeliveries)
                ));
            } else if node.phase == Phase::Auditing {
                store.commit(&upgrade).unwrap();
            }
        }
        let progressed = controller
            .drive_once_with(|point, key| {
                let point = format!("{point:?}");
                if point == selected {
                    probe(&point, key);
                }
            })
            .unwrap();
        if !progressed {
            let snapshot = controller.snapshot().unwrap();
            let pending = !snapshot.pending_effects.is_empty()
                || snapshot.rows.iter().any(|r| r.table == "watches");
            if pending {
                continue;
            }
            assert!(node.phase.is_closed() || scenario == "permanent");
            println!("{}", json!({"done": true}));
            return;
        }
    }
    panic!("seed={seed}, scenario={scenario}: failed to settle");
}

#[derive(Debug, PartialEq)]
struct Durable {
    snapshot: tl3_store::Snapshot,
    effects: Vec<tl3_store::Effect>,
    events: Vec<tl3_store::EventRecord>,
}
fn durable(settings: &tl3_config::Store) -> Durable {
    let mut store = Store::open_read_only(settings).unwrap();
    Durable {
        snapshot: store.load().unwrap(),
        effects: store.effects().unwrap(),
        events: store.events_after(Revision::ZERO, u32::MAX).unwrap(),
    }
}
fn wait_for_disconnect(settings: &tl3_config::Store) {
    let mut client = raw(settings);
    client
        .query_one("SELECT pg_advisory_lock($1)", &[&0x544c33434f4e5452_i64])
        .unwrap();
    client
        .query_one("SELECT pg_advisory_unlock($1)", &[&0x544c33434f4e5452_i64])
        .unwrap();
}

struct ControllerProcess(std::process::Child);
impl Drop for ControllerProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl std::ops::Deref for ControllerProcess {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for ControllerProcess {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

fn run(
    settings: &tl3_config::Store,
    root: &Path,
    scenario: &str,
    selected: &str,
    seed: u64,
) -> BTreeSet<String> {
    let config = settings_file(settings, root);
    let upstream = root.join("upstreams");
    let mut killed = BTreeSet::new();
    for _ in 0..1000 {
        let mut child = ControllerProcess(
            Command::new(env::current_exe().unwrap())
                .arg("--child")
                .arg(&config)
                .arg(&upstream)
                .arg(scenario)
                .arg(selected)
                .arg(seed.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert_ne!(
                output.read_line(&mut line).unwrap(),
                0,
                "seed={seed} scenario={scenario} point={selected}: child exited {:?}",
                child.wait().unwrap()
            );
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["done"] == true {
                assert!(child.wait().unwrap().success());
                return killed;
            }
            let key = message["key"].as_str().unwrap().to_owned();
            assert_eq!(message["point"], selected);
            if killed.insert(key.clone()) {
                let before = durable(settings);
                // At an interior save point Postgres must still see a live
                // transaction. SIGKILL, not Rust destructors, ends that session.
                if !matches!(
                    selected,
                    "AfterCommit"
                        | "InternAfterCommit"
                        | "BeforeDispatch"
                        | "AfterDispatch"
                        | "BeforeReconcile"
                        | "AfterReconcile"
                ) {
                    assert!(raw(settings).query_one("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = current_database() AND pid <> pg_backend_pid() AND state = 'idle in transaction')", &[]).unwrap().get::<_, bool>(0));
                }
                child.kill().unwrap();
                let status = child.wait().unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    assert_eq!(status.signal(), Some(9));
                }
                wait_for_disconnect(settings);
                assert_eq!(
                    durable(settings),
                    before,
                    "seed={seed} scenario={scenario} point={selected} key={key}: torn commit"
                );
                break;
            }
            writeln!(input, "continue").unwrap();
        }
    }
    panic!("seed={seed} scenario={scenario} point={selected}: too many crashes");
}

fn emission_coverage(
    controller: &mut Controller,
    settings: &tl3_config::Store,
) -> BTreeMap<String, (String, String)> {
    let final_world = controller.world().unwrap();
    let mut world = tl3_kernel::World::new(&[1], 1);
    // The audit oracle supplies the dictionary-backed footprints before replay.
    // Production recovery only loads snapshots and never runs this code.
    for files in final_world.file_sets() {
        world.intern_footprint(files.iter().copied());
    }
    let mut store = Store::open_read_only(settings).unwrap();
    let mut coverage = BTreeMap::new();
    for record in store.events_after(Revision::ZERO, u32::MAX).unwrap() {
        let Some(value) = record.receipt.result.get("kernel_event") else {
            continue;
        };
        let event: Event = serde_json::from_value(value.clone()).unwrap();
        let (next, outputs) = tl3_kernel::step(world, event);
        world = next;
        for (index, output) in outputs.into_iter().enumerate() {
            let (key, node, kind) = match output {
                Out::Effect {
                    node, seq, kind, ..
                } => (
                    format!("effect:{}:{seq}", node.0),
                    node,
                    format!("{kind:?}"),
                ),
                Out::Notice { node, notice } => (
                    format!("notice:{}:{index}", record.receipt.revision.get()),
                    node,
                    format!("Notice({notice:?})"),
                ),
            };
            assert!(
                coverage
                    .insert(
                        key,
                        (kind, world.nodes[node.0 as usize].phase.name().into())
                    )
                    .is_none()
            );
        }
    }
    assert_eq!(
        serde_json::to_value(world).unwrap(),
        serde_json::to_value(final_world).unwrap(),
        "snapshot differs from the audit oracle"
    );
    assert_eq!(
        coverage.keys().cloned().collect::<BTreeSet<_>>(),
        store
            .effects()
            .unwrap()
            .iter()
            .map(|e| e.key.clone())
            .collect(),
        "kernel outputs and durable outbox differ"
    );
    coverage
}

fn verify_cuts(
    settings: &tl3_config::Store,
    root: &Path,
    selected: &str,
    killed: &BTreeSet<String>,
) {
    assert!(
        !killed.is_empty(),
        "save point {selected} was never reached"
    );
    let mut store = Store::open_read_only(settings).unwrap();
    if matches!(
        selected,
        "EventWritten"
            | "RowsWritten"
            | "EffectsWritten"
            | "TransitionsWritten"
            | "CreditWritten"
            | "BeforeCommit"
            | "AfterCommit"
    ) {
        for event in store.events_after(Revision::ZERO, u32::MAX).unwrap() {
            assert!(
                killed.contains(&cut_key(&event.key, &event.payload)),
                "{selected}: committed event {} escaped injection",
                event.key
            );
        }
    } else if selected.starts_with("Intern") {
        for entry in store.load().unwrap().interned {
            let kind = match entry.kind {
                InternKind::Repo => "Repo",
                InternKind::Commit => "Commit",
                InternKind::Path => "Path",
                InternKind::Issue => "Issue",
                InternKind::PullRequest => "PullRequest",
            };
            let key = format!("intern:{kind}:{}:{}", entry.scope, entry.value);
            assert!(
                killed.contains(&key),
                "{selected}: dictionary write {key} escaped injection"
            );
        }
    } else {
        for upstream in upstreams::UPSTREAMS {
            for (key, record) in
                upstreams::load(&upstreams::path(&root.join("upstreams"), upstream)).records
            {
                if matches!(selected, "BeforeDispatch" | "AfterDispatch") || record.probes > 0 {
                    assert!(
                        killed.contains(&key),
                        "{selected}: upstream operation {key} escaped injection"
                    );
                }
            }
        }
    }
}

fn normalized(settings: &tl3_config::Store, root: &Path, scenario: &str) -> Value {
    let mut controller = Controller::open(
        Store::open(settings).unwrap(),
        upstreams::registry(&root.join("upstreams"), scenario, 0),
        bootstrap(),
    )
    .unwrap();
    let snapshot = controller.snapshot().unwrap();
    assert!(snapshot.pending_effects.is_empty());
    assert!(
        snapshot
            .rows
            .iter()
            .all(|r| !matches!(r.table.as_str(), "inbox" | "watches"))
    );
    let world = controller.world().unwrap();
    let expected = if scenario.starts_with("stop-") {
        Phase::Closed(Outcome::Stopped)
    } else if scenario == "permanent" {
        Phase::AwaitingRob
    } else {
        Phase::Closed(Outcome::Merged)
    };
    assert_eq!(world.nodes[0].phase, expected);
    let mut value = serde_json::to_value(&world).unwrap();
    for node in value["nodes"].as_array_mut().unwrap() {
        for field in ["head", "approved", "merged"] {
            if let Some(id) = node[field].as_i64() {
                node[field] = json!(
                    snapshot
                        .interned
                        .iter()
                        .find(|e| e.kind == InternKind::Commit && e.id == id)
                        .unwrap()
                        .value
                );
            }
        }
    }
    for set in value["file_sets"].as_array_mut().unwrap() {
        for id in set.as_array_mut().unwrap() {
            *id = json!(
                snapshot
                    .interned
                    .iter()
                    .find(|e| e.kind == InternKind::Path && e.id == id.as_i64().unwrap())
                    .unwrap()
                    .value
            );
        }
    }
    let coverage = emission_coverage(&mut controller, settings);
    let effects = Store::open_read_only(settings).unwrap().effects().unwrap();
    let mut outputs = vec![];
    let mut observed = BTreeSet::new();
    let mut resources = BTreeMap::new();
    let mut deduplicated = 0;
    for upstream in upstreams::UPSTREAMS {
        let state = upstreams::load(&upstreams::path(&root.join("upstreams"), upstream));
        resources.insert(format!("{upstream:?}"), json!(state.resources));
        deduplicated += state.removed_duplicates;
        for (key, record) in state.records {
            assert!(record.applications <= 1, "doubled effect {key}");
            assert!(record.copies <= 1, "duplicate message {key}");
            let effect = effects.iter().find(|e| e.key == key).unwrap();
            assert!(!effect.state.is_pending());
            observed.insert(key);
        }
    }
    assert!(
        deduplicated > 0,
        "rare duplicate-post race was not exercised"
    );
    for effect in effects {
        let delivery: Delivery = serde_json::from_value(effect.payload).unwrap();
        if effect.state != EffectState::Cancelled {
            assert!(observed.contains(&effect.key), "lost effect {}", effect.key);
        }
        let label = match delivery.output {
            Out::Effect { kind, .. } => format!("{kind:?}"),
            Out::Notice { notice, .. } => format!("Notice({notice:?})"),
        };
        outputs.push(json!([
            label,
            coverage[&effect.key].1,
            format!("{:?}", effect.state),
            effect.epoch.get(),
            delivery.node().0,
            delivery.generation,
            delivery.head_sha,
            delivery.paths,
            delivery.attempt,
            delivery.round,
            delivery.pull_request,
            delivery.issue
        ]));
    }
    outputs.sort_by_key(Value::to_string);
    let mut credit: Vec<_> = snapshot
        .credit
        .iter()
        .map(|r| json!([r.entry.node, r.entry.root, r.entry.amount]))
        .collect();
    credit.sort_by_key(Value::to_string);
    json!({"world":value, "outputs": outputs, "credit":credit, "resources":resources, "epoch":snapshot.epoch.get()})
}

fn main() {
    let args: Vec<_> = env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--child") {
        child(&args);
        return;
    }
    let seed: u64 = env::var("TL3_SIM_SEED")
        .map(|s| s.parse().expect("TL3_SIM_SEED must be u64"))
        .unwrap_or(0x16_2026);
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("crash simulation failed; replay with TL3_SIM_SEED={seed}\n{info}")
    }));
    let started = Instant::now();
    upstreams::verify_contracts();
    let mut database = support::Database::start();
    let mut total = 0;
    let mut kinds = BTreeSet::new();
    let filter = env::var("TL3_SIM_SCENARIO").ok();
    let point_filter = env::var("TL3_SIM_POINT").ok();
    for scenario in SCENARIOS
        .iter()
        .copied()
        .filter(|s| filter.as_deref().is_none_or(|f| f == *s))
    {
        let baseline_settings =
            database.create(&format!("baseline_{}", scenario.replace('-', "_")));
        let baseline_root = tempfile::tempdir().unwrap();
        run(
            &baseline_settings,
            baseline_root.path(),
            scenario,
            "none",
            seed,
        );
        let baseline = normalized(&baseline_settings, baseline_root.path(), scenario);
        for output in baseline["outputs"].as_array().unwrap() {
            kinds.insert((
                output[0].as_str().unwrap().to_owned(),
                output[1].as_str().unwrap().to_owned(),
            ));
        }
        for (index, point) in POINTS
            .iter()
            .enumerate()
            .filter(|(_, p)| point_filter.as_deref().is_none_or(|f| f == **p))
        {
            let settings =
                database.create(&format!("crash_{}_{index}", scenario.replace('-', "_")));
            let root = tempfile::tempdir().unwrap();
            let kills = run(&settings, root.path(), scenario, point, seed);
            let actual = normalized(&settings, root.path(), scenario);
            verify_cuts(&settings, root.path(), point, &kills);
            assert_eq!(
                actual, baseline,
                "seed={seed} scenario={scenario} point={point}: recovery differs from uninterrupted run"
            );
            total += kills.len();
            println!(
                "seed={seed} scenario={scenario} point={point}: {} SIGKILL cuts, recovered equal to baseline",
                kills.len()
            );
        }
    }
    assert!(total > 0, "filter selected no crash points");
    if filter.is_none() {
        for (kind, phase) in [
            ("StartRun(Ingest)", "Ingesting"),
            ("StartRun(Audit)", "Auditing"),
            ("StartRun(Plan)", "Planning"),
            ("StartRun(Make)", "Making"),
            ("StartRun(Review)", "Reviewing"),
            ("SubmitChecks", "Checking"),
            ("StartRebase", "Rebasing"),
            ("PrepareWorktree", "Making"),
            ("Push", "Publishing"),
            ("PostApproval", "Publishing"),
            ("OpenPr", "Publishing"),
            ("Enqueue", "Publishing"),
            ("StopRun", "Stopping"),
            ("WithdrawJob", "Stopping"),
            ("ClosePr", "Stopping"),
            ("RemoveWorktree", "Stopping"),
        ] {
            assert!(
                kinds.contains(&(kind.into(), phase.into())),
                "missing effect/phase coverage: {kind} in {phase}"
            );
        }
    }
    println!("effect/phase coverage: {kinds:?}");
    database.stop();
    println!(
        "{total} real SIGKILL cuts passed in {:.2}s; seed={seed}; no skips",
        started.elapsed().as_secs_f64()
    );
}
