#[path = "../../store/tests/support/mod.rs"]
mod support;
use std::{
    collections::BTreeSet,
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
    executor::{Registry, Upstream},
    status,
};
use tl3_kernel::{Outcome, Phase};
use tl3_store::{EffectState, Reader, Store};

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
    sent_but_absent(&database.create("absent"));
    acknowledged_operation_look(&database.create("look"));
    retry_uses_the_kernels_new_key(&database.create("retry"));
    println!("test restart_without_duplicate_deliveries ... ok");
    status_page_and_command(&database.create("status"));
    println!("test status_page_and_command ... ok");
    database.stop();
    println!(
        "2 required controller scenarios, 3 recovery edge cases and status HTTP simulation passed; no skips"
    );
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
    assert_finished(&mut controller, &records);
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
    assert!(cli_status(&config).contains("Closed: merged\tnone\t5"));
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
                    if boundary
                        == if args[4] == "picked_up" {
                            Boundary::BeforeDispatch
                        } else {
                            Boundary::AfterDispatch
                        }
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
    kill_at(&config, &records, "picked_up");
    assert!(adapters::delivery_counts(&records).unwrap().is_empty());
    assert_eq!(
        Store::open(settings)
            .unwrap()
            .load()
            .unwrap()
            .pending_effects[0]
            .state,
        EffectState::Sent
    );
    let mut controller = open(settings, &records);
    drain(&mut controller);
    assert_finished(&mut controller, &records);
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
    let mut controller = open(settings, root.path());
    drain(&mut controller);
    assert_finished(&mut controller, root.path());
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
    assert_finished(&mut controller, root.path());
    assert_eq!(
        controller.effect("effect:0:0").unwrap().unwrap().state,
        EffectState::Cancelled
    );
    assert!(calls.borrow().values().all(|count| *count == 1));
}

struct StatusServer(Child);
impl Drop for StatusServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn http(address: std::net::SocketAddr, method: &str, path: &str) -> (String, String) {
    let mut connection = TcpStream::connect(address).unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(connection, "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 0\r\n\r\n").unwrap();
    let mut response = String::new();
    connection.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    (headers.to_ascii_lowercase(), body.into())
}

fn status_page_and_command(settings: &tl3_config::Store) {
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
    assert_eq!(row.waiting_on, "worktree preparation");
    assert_eq!(row.credit, 4);
    assert_eq!(row.details, status::RequestDetails::default());
    let expected_text = concat!(
        "Request\tPhase\tWaiting on\tCredit\tLast progress\tOpen incidents\n",
        "RobTand/tl3-sandbox#42: Fix <script>alert('oops')</script> & spacing\t",
        "Making the change\tworktree preparation\t4\tnone recorded\tnone recorded\n"
    );
    assert_eq!(status::render_text(&model), expected_text);
    let expected_html = status::render_html(&model);
    assert!(expected_html.contains("<td>RobTand/tl3-sandbox#42: Fix &lt;script&gt;alert(&#39;oops&#39;)&lt;/script&gt; &amp; spacing</td>"));
    assert!(expected_html.contains("<td>Making the change</td><td>worktree preparation</td><td>4</td><td>none recorded</td><td>none recorded</td>"));
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
    assert_eq!(body, expected_html);
    let (headers, body) = http(address, "HEAD", "/");
    assert!(headers.starts_with("http/1.1 200"));
    assert!(headers.contains(&format!("content-length: {}", expected_html.len())));
    assert!(body.is_empty());
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
}
