use postgresql_embedded::{PostgreSQL, Settings, VersionReq};
use std::{
    collections::{BTreeSet, HashMap},
    env, fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tempfile::TempDir;
use tl3_controller::{
    Bootstrap, Boundary, Controller, LabelledIssue, Repository, adapters,
    executor::{Registry, Upstream},
    status,
};
use tl3_kernel::{Outcome, Phase};
use tl3_store::{EffectState, Store};

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
fn labelled() -> LabelledIssue {
    LabelledIssue {
        delivery: "fixture-delivery".into(),
        owner: "RobTand".into(),
        repo: "tl3-sandbox".into(),
        installation: 1,
        number: 42,
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

fn main() {
    let args: Vec<_> = env::args().collect();
    if args.get(1).is_some_and(|s| s == "--child") {
        child(&args);
        return;
    }
    let mut database = Database::start();
    labelled_issue_closes(&database.create("clean"));
    println!("test labelled_issue_closes ... ok");
    restart_without_duplicate_deliveries(&database.create("restart"));
    println!("test restart_without_duplicate_deliveries ... ok");
    database.stop();
    println!("2 controller integration scenarios passed; no skips");
}

struct Database {
    postgres: PostgreSQL,
    runtime: tokio::runtime::Runtime,
    scratch: TempDir,
    password_file: PathBuf,
}

impl Database {
    fn start() -> Self {
        Self::start_with_fsync(true)
    }

    fn start_with_fsync(fsync: bool) -> Self {
        // Every archive, executable, cluster and fixture credential belongs to this job.
        let scratch = tempfile::Builder::new()
            .prefix("tl3-controller-pg-")
            .tempdir()
            .unwrap();
        let root = scratch.path();
        let settings = Settings {
            releases_url: "https://github.com/zonkyio/embedded-postgres-binaries".into(),
            version: VersionReq::parse("=17.4.0").unwrap(),
            installation_dir: root.join("binaries"),
            password_file: root.join("pgpass"),
            data_dir: root.join("data"),
            host: "127.0.0.1".into(),
            username: "postgres".into(),
            password: "tl3-disposable-test-password".into(),
            timeout: Some(Duration::from_secs(90)),
            configuration: HashMap::from([
                ("max_connections".into(), "20".into()),
                ("shared_buffers".into(), "16MB".into()),
                ("max_parallel_workers".into(), "0".into()),
                ("max_worker_processes".into(), "0".into()),
                // postgresql_embedded supplies -F; override it for every recovery test.
                ("fsync".into(), if fsync { "on" } else { "off" }.into()),
                ("synchronous_commit".into(), "on".into()),
            ]),
            ..Settings::default()
        };
        let mut postgres = PostgreSQL::new(settings);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(2)
            .build()
            .unwrap();
        let started = Instant::now();
        runtime
            .block_on(postgres.setup())
            .expect("download and initialize embedded Postgres");
        println!(
            "embedded Postgres cold setup (fsync={fsync}): {:.2}s",
            started.elapsed().as_secs_f64()
        );
        runtime
            .block_on(postgres.start())
            .expect("start embedded Postgres");
        let password_file = root.join("client-password");
        fs::write(&password_file, "tl3-disposable-test-password\n").unwrap();
        Self {
            postgres,
            runtime,
            scratch,
            password_file,
        }
    }

    fn create(&mut self, name: &str) -> tl3_config::Store {
        self.runtime
            .block_on(self.postgres.create_database(name))
            .unwrap();
        tl3_config::Store {
            host: "127.0.0.1".into(),
            port: self.postgres.settings().port,
            database: name.into(),
            user: "postgres".into(),
            password_file: Some(self.password_file.clone()),
            connect_timeout_secs: 10,
        }
    }

    fn stop(&self) {
        self.runtime.block_on(self.postgres.stop()).unwrap();
        assert!(self.scratch.path().exists());
    }
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
        initial.contains("Ingesting\tStartRun(Ingest)\t0"),
        "{initial}"
    );
    let mut seen = BTreeSet::new();
    for iteration in 0..200 {
        let world = controller.world().unwrap();
        assert!(world.valid() && world.is_settled());
        seen.insert(world.nodes[0].phase.name());
        let mut reader = Store::open(settings).unwrap();
        let rendered = status::render(&mut reader).unwrap();
        assert!(rendered.contains(&format!("\t{}\n", world.nodes[0].credit.balance())));
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
    assert_finished(&mut controller, &records);
    let final_status = cli_status(&config);
    assert!(
        final_status.contains("RobTand/tl3-sandbox#42\tClosed(Merged)\tnone\t5"),
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
    let mut restored = open(settings, &records);
    assert_eq!(restored.snapshot().unwrap(), before);
    assert!(!restored.drive_once().unwrap());
}

fn assert_finished(controller: &mut Controller, root: &Path) {
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
    kill_at(&config, &records, "dispatched");
    let snapshot = Store::open(settings).unwrap().load().unwrap();
    assert_eq!(snapshot.pending_effects[0].state, EffectState::Sent);
    assert_eq!(
        adapters::delivery_counts(&records)
            .unwrap()
            .values()
            .sum::<u64>(),
        1
    );
    let mut controller = open(settings, &records);
    for iteration in 0..200 {
        if !controller.drive_once().unwrap() {
            break;
        }
        assert!(iteration < 199);
    }
    assert_finished(&mut controller, &records);
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
    assert_finished(&mut open(settings, &records), &records);
    assert!(cli_status(&config).contains("Closed(Merged)\tnone\t5"));
}

fn kill_at(config: &Path, records: &Path, boundary: &str) {
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
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success());
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
    }
}
fn child(args: &[String]) {
    let config = tl3_config::Config::parse(&fs::read_to_string(&args[2]).unwrap()).unwrap();
    let mut controller = open(&config.store, Path::new(&args[3]));
    if args[4] == "committed" {
        controller.intake(labelled()).unwrap();
        park();
    }
    loop {
        assert!(
            controller
                .drive_once_with(|boundary, _| {
                    if boundary == Boundary::AfterDispatch {
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
