//! Console commands (#331, PRD #327 C3 and C8): each typed command has the
//! effect of Rob's equivalent comment, refusals say why, and a crash between
//! filing and applying loses nothing and applies once.

#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::{Value, json};
use std::{
    cell::Cell,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    rc::Rc,
};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    executor::{Completion, Delivery, Executor, Look, Observation, Reconciliation, Upstream},
};
use tl3_kernel::{Event, NodeId, Outcome, Phase, QKind, Reply};
use tl3_store::{ConsoleCommands, SavePoint, Store};

const ROB: u64 = 42;

fn repository() -> Repository {
    Repository {
        owner: "owner".into(),
        name: "sandbox".into(),
        installation: 3,
    }
}

/// Workers that start runs and never reply, or fail them permanently so a
/// question opens.
#[derive(Clone, Default)]
struct Workers {
    fail: Rc<Cell<bool>>,
    unreadable: Rc<Cell<bool>>,
}

impl Executor for Workers {
    fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
        Ok(if self.fail.get() {
            Completion::permanent("fixture")
        } else {
            Completion::confirmed(vec![])
        })
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(Reconciliation::Pending)
    }
    fn look(&mut self, _: &Delivery) -> tl3_controller::Result<Look> {
        if self.unreadable.get() {
            return Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into());
        }
        Ok(Look::Healthy)
    }
}

struct World {
    controller: Controller,
    settings: tl3_config::Store,
    root: tempfile::TempDir,
    workers: Workers,
}

fn open(settings: &tl3_config::Store, root: &Path, workers: &Workers, store: Store) -> Controller {
    open_clocked(settings, root, workers, store, false)
}

/// A clock an hour ahead at every reading, so each failed look counts (#356).
struct Hours(Cell<u64>);

impl tl3_controller::clock::Clock for Hours {
    fn now(&self) -> std::time::SystemTime {
        self.0.set(self.0.get() + 3_600_000);
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(self.0.get())
    }
}

fn open_clocked(
    settings: &tl3_config::Store,
    root: &Path,
    workers: &Workers,
    store: Store,
    hours: bool,
) -> Controller {
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(Upstream::Workers, workers.clone());
    let _ = settings;
    let bootstrap = Bootstrap {
        repos: vec![repository()],
        rob: ROB,
        slots: 2,
        pool: 2,
        operations: None,
    };
    if hours {
        Controller::open_with_clock(
            store,
            registry,
            bootstrap,
            Hours(Cell::new(1_800_000_000_000)),
        )
        .unwrap()
    } else {
        Controller::open(store, registry, bootstrap).unwrap()
    }
}

impl World {
    /// Requests for the given issues, each driven until its worker run waits.
    fn new(db: &mut support::Database, name: &str, issues: &[u64], fail: bool) -> Self {
        Self::clocked(db, name, issues, fail, false)
    }

    fn clocked(
        db: &mut support::Database,
        name: &str,
        issues: &[u64],
        fail: bool,
        hours: bool,
    ) -> Self {
        let settings = db.create(name);
        let root = tempfile::tempdir().unwrap();
        let workers = Workers::default();
        workers.fail.set(fail);
        let mut controller = open_clocked(
            &settings,
            root.path(),
            &workers,
            Store::open(&settings).unwrap(),
            hours,
        );
        for issue in issues {
            controller
                .intake(LabelledIssue {
                    delivery: format!("label-{issue}"),
                    owner: "owner".into(),
                    repo: "sandbox".into(),
                    installation: 3,
                    number: *issue,
                    title: format!("Fix greeting {issue}"),
                    actor: ROB,
                    label: "tl3".into(),
                })
                .unwrap();
        }
        let mut world = World {
            controller,
            settings,
            root,
            workers,
        };
        world.settle();
        world
    }

    fn settle(&mut self) {
        while self.controller.drive_once().unwrap() {}
    }

    fn node(&mut self, node: u64) -> tl3_kernel::Node {
        self.controller.world().unwrap().nodes[node as usize]
    }

    /// Rob's issue comment, classified as `reply`.
    fn comment(&mut self, id: u64, issue: u64, body: &str, reply: Value) {
        self.controller
            .observe(
                &format!("comment-{id}"),
                Observation::GitHubComment {
                    repository: repository(),
                    issue,
                    comment: tl3_github::Comment {
                        id,
                        body: body.into(),
                        user: tl3_github::Actor {
                            id: ROB,
                            login: "rob".into(),
                        },
                        created_at: chrono::DateTime::UNIX_EPOCH,
                        updated_at: chrono::DateTime::UNIX_EPOCH,
                        issue_url: format!(
                            "https://api.github.com/repos/owner/sandbox/issues/{issue}"
                        ),
                    },
                },
            )
            .unwrap();
        self.settle();
        if reply == Value::Null {
            return;
        }
        let key = self
            .controller
            .snapshot()
            .unwrap()
            .rows
            .into_iter()
            .find(|row| row.table == "comment_jobs")
            .map(|row| row.id)
            .expect("the comment waits for its classifier");
        self.controller
            .observe(
                &format!("classified-{id}"),
                Observation::Classified {
                    key,
                    reply: Some(reply),
                },
            )
            .unwrap();
        self.settle();
    }

    /// A console command, filed the way the console files it, then drained.
    fn console(&mut self, id: &str, node: u64, action: Value) -> bool {
        let filed = ConsoleCommands::open(&self.settings)
            .unwrap()
            .submit(id, node, &action)
            .unwrap();
        tl3_controller::ingress::drain(&mut self.controller).unwrap();
        self.settle();
        filed
    }

    fn outcome(&self, id: &str) -> (String, Option<String>) {
        let mut client = raw(&self.settings);
        let row = client
            .query_one(
                "SELECT status, reason FROM tl3_console_commands WHERE id = $1",
                &[&id],
            )
            .unwrap();
        (row.get(0), row.get(1))
    }

    fn notes(&self) -> Vec<String> {
        adapters::delivered_notes(self.root.path()).unwrap()
    }

    fn rows(&mut self, table: &str) -> Vec<(String, Value)> {
        self.controller
            .snapshot()
            .unwrap()
            .rows
            .into_iter()
            .filter(|row| row.table == table)
            .map(|row| (row.id, row.value))
            .collect()
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

fn uuid(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}

/// What Rob's commands change: the kernel node, and the replacement a
/// restart or changed request leaves.
fn effect(world: &mut World, node: u64) -> (tl3_kernel::Node, Vec<String>) {
    let tasks = world
        .rows("replacement")
        .into_iter()
        .chain(world.rows("replacement_pending"))
        .map(|(id, value)| {
            let (task, fields) = value
                .as_object()
                .and_then(|task| task.iter().next())
                .map(|(task, fields)| (task.clone(), fields.clone()))
                .unwrap_or_default();
            // A changed request's text is its effect; a restart's quotes its trigger.
            if task == "FileRequest" {
                format!("{id}:{task}:{}", fields["text"])
            } else {
                format!("{id}:{task}")
            }
        })
        .collect();
    (world.node(node), tasks)
}

/// One console command against Rob's comment with the same meaning, each in a
/// fresh store with the same request.
fn matches_comment(name: &str, body: &str, kind: &str, note: &str) {
    let mut db = support::Database::start();
    let mut comment = World::new(&mut db, &format!("comment_{name}"), &[12], false);
    let mut console = World::new(&mut db, &format!("console_{name}"), &[12], false);
    if kind == "resume" {
        comment.comment(900, 12, "Please pause", json!({"kind": "pause"}));
        assert!(console.console(&uuid(900), 0, json!({"kind": "pause"})));
    }
    let before = effect(&mut console, 0);
    comment.comment(1000, 12, body, json!({ "kind": kind }));
    let id = uuid(1000);
    assert!(console.console(&id, 0, json!({ "kind": kind })));
    let after = effect(&mut console, 0);
    assert_ne!(after, before, "{name} changes the request");
    assert_eq!(after, effect(&mut comment, 0), "{name} matches the comment");
    assert_eq!(console.outcome(&id), ("applied".into(), None), "{name}");
    assert!(
        console.notes().iter().any(|text| text.starts_with(note)),
        "{name}: {:?}",
        console.notes()
    );
    db.stop();
}

#[test]
fn pause_matches_robs_comment() {
    matches_comment(
        "pause",
        "Please pause",
        "pause",
        "Rob paused this request from the TL3 console.",
    );
}

#[test]
fn resume_matches_robs_comment() {
    matches_comment(
        "resume",
        "Please resume",
        "resume",
        "Rob resumed this request from the TL3 console.",
    );
}

#[test]
fn stop_matches_robs_comment() {
    matches_comment(
        "stop",
        "Please stop",
        "stop",
        "Rob stopped this request from the TL3 console.",
    );
}

#[test]
fn restart_matches_robs_comment() {
    matches_comment(
        "restart",
        "Restart this from the issue",
        "restart",
        "Rob restarted this request from the TL3 console.",
    );
}

#[test]
fn answers_and_deferrals_match_robs_question_comment() {
    let mut db = support::Database::start();
    let mut comment = World::new(&mut db, "comment_answers", &[12], true);
    let mut console = World::new(&mut db, "console_answers", &[12], true);
    let question = console.node(0).question.expect("a question is open");
    assert_eq!(question.kind, QKind::EffectFailed);
    assert_eq!(console.node(0).phase, Phase::AwaitingRob);
    comment.workers.fail.set(false);
    console.workers.fail.set(false);
    // Choosing option one equals Rob's `Question N: 1` comment.
    comment.comment(
        2000,
        12,
        &format!("Question {}: 1", question.id),
        json!({"kind": "answer", "question_id": question.id.to_string(), "answer": "continue"}),
    );
    let id = uuid(2000);
    assert!(console.console(
        &id,
        0,
        json!({"kind": "answer", "question": question.id, "choice": {"answer": "continue"}})
    ));
    assert_eq!(effect(&mut console, 0), effect(&mut comment, 0));
    assert_ne!(console.node(0).phase, Phase::AwaitingRob);
    assert_eq!(console.outcome(&id), ("applied".into(), None));
    assert!(console.notes().iter().any(|text| text
        == &format!(
            "Rob answered question {} from the TL3 console: Continue after fixing the cause.",
            question.id
        )));

    // Deferring applies the recommended option, and the note says Rob deferred.
    let mut deferred = World::new(&mut db, "console_defer", &[12], true);
    deferred.workers.fail.set(false);
    let id = uuid(2001);
    assert!(deferred.console(&id, 0, json!({"kind": "defer", "question": question.id})));
    assert_eq!(effect(&mut deferred, 0), effect(&mut comment, 0));
    assert_eq!(deferred.outcome(&id), ("applied".into(), None));
    assert!(deferred.notes().iter().any(|text| text
        == &format!(
            "Rob deferred to TL3 on question {}. TL3 applied its recommended option: Continue after fixing the cause.",
            question.id
        )));
    db.stop();
}

#[test]
fn bulk_defer_applies_each_recommendation_once_and_skips_answered_questions() {
    let mut db = support::Database::start();
    let mut world = World::new(&mut db, "bulk_defer", &[12, 13, 14], true);
    world.workers.fail.set(false);
    let questions: Vec<_> = (0..3)
        .map(|node| world.node(node).question.expect("a question is open").id)
        .collect();
    // Question three is answered before the bulk defer arrives.
    assert!(world.console(
        &uuid(3000),
        2,
        json!({"kind": "answer", "question": questions[2], "choice": {"answer": "continue"}})
    ));
    assert_ne!(world.node(2).phase, Phase::AwaitingRob);
    let ids: Vec<_> = (0..3).map(|n| uuid(3001 + n)).collect();
    let file = |world: &mut World| -> Vec<bool> {
        let mut commands = ConsoleCommands::open(&world.settings).unwrap();
        (0..3)
            .map(|node| {
                commands
                    .submit(
                        &ids[node],
                        node as u64,
                        &json!({"kind": "defer", "question": questions[node]}),
                    )
                    .unwrap()
            })
            .collect()
    };
    assert_eq!(file(&mut world), [true, true, true]);
    tl3_controller::ingress::drain(&mut world.controller).unwrap();
    world.settle();
    let last_cmds: Vec<_> = (0..3).map(|node| world.node(node).last_cmd).collect();
    for (node, id) in ids.iter().enumerate().take(2) {
        assert_eq!(world.outcome(id), ("applied".into(), None));
        assert_ne!(world.node(node as u64).phase, Phase::AwaitingRob);
    }
    assert_eq!(
        world.outcome(&ids[2]),
        (
            "refused".into(),
            Some(format!("Question {} was answered meanwhile.", questions[2]))
        )
    );
    // Confirming the same list again files nothing more.
    assert_eq!(file(&mut world), [false, false, false]);
    tl3_controller::ingress::drain(&mut world.controller).unwrap();
    world.settle();
    let again: Vec<_> = (0..3).map(|node| world.node(node).last_cmd).collect();
    assert_eq!(again, last_cmds);
    let deferrals = world
        .notes()
        .into_iter()
        .filter(|text| text.starts_with("Rob deferred to TL3"))
        .count();
    assert_eq!(deferrals, 2);
    db.stop();
}

#[test]
fn refusals_say_why_and_change_nothing() {
    let mut db = support::Database::start();
    let mut world = World::new(&mut db, "refusals", &[12], false);
    assert!(world.console(&uuid(4000), 0, json!({"kind": "pause"})));
    let cases = [
        (
            json!({"kind": "pause"}),
            0,
            "The request is already paused.",
        ),
        (
            json!({"kind": "stop", "extra": 1}),
            0,
            "TL3 couldn't read this command.",
        ),
        (
            json!({"kind": "launch"}),
            0,
            "TL3 couldn't read this command.",
        ),
        (json!({"kind": "pause"}), 7, "There is no request 7."),
        (
            json!({"kind": "answer", "question": 99, "choice": {"answer": "continue"}}),
            0,
            "Question 99 was answered meanwhile.",
        ),
        (
            json!({"kind": "defer", "question": 99}),
            0,
            "Question 99 was answered meanwhile.",
        ),
        (
            json!({"kind": "clear_incident", "incident": "nope"}),
            0,
            "There is no incident nope.",
        ),
        (
            json!({"kind": "comment", "text": "  "}),
            0,
            "Write an answer first.",
        ),
    ];
    for (index, (action, node, reason)) in cases.into_iter().enumerate() {
        let before = world.controller.world().unwrap();
        let notes = world.notes().len();
        let id = uuid(4001 + index as u32);
        assert!(world.console(&id, node, action.clone()));
        assert_eq!(
            world.outcome(&id),
            ("refused".into(), Some(reason.into())),
            "{action}"
        );
        assert_eq!(world.controller.world().unwrap(), before, "{action}");
        assert_eq!(world.notes().len(), notes, "{action}");
    }
    // A request that merged can't restart (Q49), and a closed one can't pause.
    assert!(world.console(&uuid(4100), 0, json!({"kind": "stop"})));
    world.settle();
    assert_eq!(world.node(0).phase, Phase::Closed(Outcome::Stopped));
    assert!(world.console(&uuid(4101), 0, json!({"kind": "resume"})));
    assert_eq!(
        world.outcome(&uuid(4101)),
        ("refused".into(), Some("The request is closed.".into()))
    );
    db.stop();
}

#[test]
fn clear_incident_matches_robs_comment() {
    let mut db = support::Database::start();
    let mut worlds = Vec::new();
    for name in ["comment_clear", "console_clear"] {
        let mut world = World::clocked(&mut db, name, &[12], false, true);
        // Two unreadable looks at the worker run open an incident on the request.
        let (key, _) = world
            .rows("allowances")
            .into_iter()
            .find(|(key, _)| key.starts_with("effect:0:"))
            .expect("a worker run is in flight");
        world.workers.unreadable.set(true);
        for _ in 0..10 {
            world.controller.look_key(&key).unwrap();
            if world
                .rows("incidents")
                .iter()
                .any(|(_, value)| value["open"] == true)
            {
                break;
            }
        }
        world.workers.unreadable.set(false);
        world.settle();
        worlds.push(world);
    }
    let incident = |world: &mut World| -> (String, Value) {
        world
            .rows("incidents")
            .into_iter()
            .next()
            .expect("an incident is open")
    };
    let (id, _) = incident(&mut worlds[0]);
    let (console_id, _) = incident(&mut worlds[1]);
    assert_eq!(id, console_id);
    worlds[0].comment(5000, 12, &format!("clear incident {id}"), Value::Null);
    assert!(worlds[1].console(
        &uuid(5000),
        0,
        json!({"kind": "clear_incident", "incident": id})
    ));
    let (comment, console) = worlds.split_at_mut(1);
    assert_eq!(effect(&mut console[0], 0), effect(&mut comment[0], 0));
    assert_eq!(
        incident(&mut console[0]).1["open"],
        incident(&mut comment[0]).1["open"]
    );
    assert_eq!(console[0].outcome(&uuid(5000)), ("applied".into(), None));
    assert!(console[0].console(
        &uuid(5001),
        0,
        json!({"kind": "clear_incident", "incident": id})
    ));
    assert_eq!(
        console[0].outcome(&uuid(5001)),
        (
            "refused".into(),
            Some(format!("Incident {id} is already cleared."))
        )
    );
    db.stop();
}

#[test]
fn own_words_reach_the_classifier_like_a_comment() {
    let mut db = support::Database::start();
    let mut world = World::new(&mut db, "own_words", &[12], false);
    let id = uuid(6000);
    assert!(world.console(
        &id,
        0,
        json!({"kind": "comment", "text": "Use the blue greeting instead"})
    ));
    assert_eq!(world.outcome(&id), ("applied".into(), None));
    let jobs = world.rows("comment_jobs");
    assert_eq!(jobs.len(), 1, "the classifier reads Rob's words");
    assert_eq!(
        jobs[0].1["steering"]["Classify"]["inputs"]["comment"]["body"],
        "Use the blue greeting instead"
    );
    assert!(world.notes().iter().any(|text| text.starts_with(
        "Rob answered from the TL3 console. TL3 reads it as a comment:\n\n    Use the blue greeting instead"
    )));
    db.stop();
}

#[test]
fn a_crash_between_filing_and_applying_loses_nothing_and_applies_once() {
    let mut db = support::Database::start();
    let mut world = World::new(&mut db, "crash", &[12], false);
    let last = world.node(0).last_cmd;
    // tl3d is down when the console files the command.
    let id = uuid(7000);
    assert!(
        ConsoleCommands::open(&world.settings)
            .unwrap()
            .submit(&id, 0, &json!({"kind": "pause"}))
            .unwrap()
    );
    assert_eq!(world.outcome(&id), ("pending".into(), None));
    // tl3d crashes after the command's commit and before ingress records it consumed.
    let mut store = Store::open(&world.settings).unwrap();
    let consumed = format!("consume-ingress:console:{id}");
    store.set_boundary_hook(move |point, key, _| {
        if point == SavePoint::ConsumeIngressBeforeCommit && key == consumed {
            panic!("crash before consuming the console command");
        }
    });
    // The fixture's controller holds the controller lock; the crashing one takes it over.
    let World {
        controller,
        settings,
        root,
        workers,
    } = world;
    drop(controller);
    let mut crashing = open(&settings, root.path(), &workers, store);
    assert!(
        catch_unwind(AssertUnwindSafe(|| tl3_controller::ingress::drain(
            &mut crashing
        )))
        .is_err()
    );
    drop(crashing);
    // The restarted controller replays the command; it changes nothing more.
    let mut world = World {
        controller: open(
            &settings,
            root.path(),
            &workers,
            Store::open(&settings).unwrap(),
        ),
        settings,
        root,
        workers,
    };
    assert_eq!(world.outcome(&id), ("applied".into(), None));
    tl3_controller::ingress::drain(&mut world.controller).unwrap();
    world.settle();
    assert!(world.node(0).paused);
    assert_eq!(world.node(0).last_cmd, last + 1);
    assert!(
        Store::open(&world.settings)
            .unwrap()
            .pending_ingress()
            .unwrap()
            .is_empty()
    );
    let notes = world
        .notes()
        .into_iter()
        .filter(|text| text.starts_with("Rob paused"))
        .count();
    assert_eq!(notes, 1);
    let mut client = raw(&world.settings);
    let commits: i64 = client
        .query_one(
            "SELECT count(*) FROM tl3_events WHERE event_key = $1",
            &[&format!("console:{id}")],
        )
        .unwrap()
        .get(0);
    assert_eq!(commits, 1);
    db.stop();
}

/// tl3d wakes on the console's notification alone: no comment, webhook or
/// stdin input arrives after the command is filed.
#[test]
fn tl3d_wakes_for_a_filed_command() {
    use std::{
        io::Write,
        process::{Command, Stdio},
        sync::mpsc,
        time::Duration,
    };
    let mut db = support::Database::start();
    let settings = db.create("daemon_wake");
    let root = tempfile::tempdir().unwrap();
    let records = root.path().join("records");
    std::fs::create_dir_all(&records).unwrap();
    // The request waits in the merge queue, so it stays open to pause.
    std::fs::write(records.join("hold-merges"), "hold").unwrap();
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
        )
        .replace(
            "/home/rob/.config/tl3/github-app.json",
            &root.path().join("github-fixture").display().to_string(),
        )
        .replace(
            "/run/tl3/model-key",
            &root.path().join("model-fixture").display().to_string(),
        );
    for name in ["github-fixture", "model-fixture"] {
        use std::os::unix::fs::PermissionsExt;
        let path = root.path().join(name);
        std::fs::write(&path, "disposable fixture").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let config_path = root.path().join("config.toml");
    std::fs::write(&config_path, config).unwrap();
    // Waits below block on the change feed, and stop early if tl3d exits.
    let (changed, changes) = mpsc::channel();
    let mut listener = tl3_store::Listener::open(&settings).unwrap();
    std::thread::spawn(move || {
        while listener.wait().is_ok() {
            if changed.send(()).is_err() {
                break;
            }
        }
    });
    // tl3d migrates on open too; readers below need the schema first.
    Store::open(&settings).unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_tl3d"))
        .arg("--config")
        .arg(&config_path)
        .arg("--simulate")
        .arg(&records)
        .arg("--console")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = daemon.stdin.take().unwrap();
    let issue = json!({"LabelledIssue": {
        "delivery": "daemon-label", "owner": "RobTand", "repo": "tl3-sandbox",
        "installation": 1, "number": 42, "title": "Fix the sandbox build",
        "actor": 24923402, "label": "tl3"
    }});
    writeln!(stdin, "{issue}").unwrap();
    stdin.flush().unwrap();
    let wait_for =
        |daemon: &mut std::process::Child, what: &str, done: &mut dyn FnMut() -> bool| {
            let mut waited = Duration::ZERO;
            while !done() {
                assert!(
                    daemon.try_wait().unwrap().is_none(),
                    "tl3d exited waiting for {what}"
                );
                assert!(
                    waited < Duration::from_secs(60),
                    "timed out waiting for {what}"
                );
                if changes.recv_timeout(Duration::from_millis(500)).is_err() {
                    waited += Duration::from_millis(500);
                }
            }
        };
    let phase = |settings: &tl3_config::Store| -> Value {
        tl3_store::Reader::open(settings)
            .unwrap()
            .load()
            .unwrap()
            .rows
            .into_iter()
            .find(|row| row.table == "nodes" && row.id == "0")
            .map(|row| row.value)
            .unwrap_or(Value::Null)
    };
    wait_for(&mut daemon, "the request to queue", &mut || {
        phase(&settings)["phase"] == "Queued"
    });
    let id = uuid(8000);
    assert!(
        ConsoleCommands::open(&settings)
            .unwrap()
            .submit(&id, 0, &json!({"kind": "pause"}))
            .unwrap()
    );
    let status = |settings: &tl3_config::Store| -> String {
        raw(settings)
            .query_one(
                "SELECT status FROM tl3_console_commands WHERE id = $1",
                &[&id],
            )
            .unwrap()
            .get(0)
    };
    wait_for(&mut daemon, "tl3d to apply the command", &mut || {
        status(&settings) != "pending"
    });
    assert_eq!(status(&settings), "applied");
    assert_eq!(phase(&settings)["paused"], true);
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    db.stop();
}

impl World {
    /// Feed the request's worker replies until the kernel asks Rob `kind`.
    fn ask(&mut self, node: u64, kind: QKind) -> u64 {
        for step in 0..60 {
            self.settle();
            let n = self.node(node);
            if let Some(question) = n.question {
                assert_eq!(question.kind, kind, "{n:?}");
                return question.id;
            }
            let generation = n.generation;
            let key = format!("drive-{node}-{step}");
            let reply = match n.phase {
                // The smallest size leaves only the reserve, so credit runs out sooner.
                Phase::Ingesting => Reply::IngestDone {
                    size: u8::from(kind != QKind::OutOfCredit),
                },
                Phase::Auditing => Reply::AuditAgrees,
                Phase::Planning { .. } if kind == QKind::NeedsSplitting => Reply::PlanSplit,
                Phase::Planning { .. } if kind == QKind::CantMeet => Reply::PlanCantMeet,
                Phase::Planning { .. } => {
                    let plan = Observation::Plan {
                        node: NodeId(node),
                        generation,
                        paths: vec!["src/greeting.py".into()],
                        fits: true,
                    };
                    self.controller.observe(&key, plan).unwrap();
                    continue;
                }
                // Each new head past the free rechecks, and each new attempt, costs credit.
                Phase::Making => {
                    let made = Observation::Made {
                        node: NodeId(node),
                        generation,
                        sha: format!("{:040x}", 5000 + step),
                    };
                    self.controller.observe(&key, made).unwrap();
                    continue;
                }
                Phase::Reviewing => Reply::ChangesRequested,
                phase => panic!("no reply drives {phase:?} toward {kind:?}"),
            };
            let event = Event::Reply {
                node: NodeId(node),
                generation,
                reply,
            };
            self.controller
                .observe(&key, Observation::Kernel(event))
                .unwrap();
        }
        panic!("the kernel never asked {kind:?}");
    }
}

/// An answer with details, from a comment and from the console, in two stores
/// that asked the same question.
fn detail_matches_comment(name: &str, kind: QKind, choice: Value, reply: Value, note: &str) {
    let mut db = support::Database::start();
    let mut comment = World::new(&mut db, &format!("comment_{name}"), &[12], false);
    let mut console = World::new(&mut db, &format!("console_{name}"), &[12], false);
    let question = comment.ask(0, kind);
    assert_eq!(console.ask(0, kind), question);
    let before = effect(&mut console, 0);
    let mut reply = reply;
    reply["kind"] = json!("answer");
    reply["question_id"] = json!(question.to_string());
    comment.comment(2100, 12, &format!("Question {question}: my answer"), reply);
    let id = uuid(2100);
    assert!(console.console(
        &id,
        0,
        json!({"kind": "answer", "question": question, "choice": choice})
    ));
    assert_eq!(console.outcome(&id), ("applied".into(), None));
    let after = effect(&mut console, 0);
    assert_ne!(after, before, "{name} changes the request");
    assert_eq!(after, effect(&mut comment, 0), "{name} matches the comment");
    assert_eq!(console.rows("findings"), comment.rows("findings"), "{name}");
    let expected = format!("Rob answered question {question} from the TL3 console: {note}");
    assert!(
        console
            .notes()
            .iter()
            .any(|text| text.starts_with(&expected)),
        "{name}: {:?}",
        console.notes()
    );
    db.stop();
}

#[test]
fn try_again_with_guidance_matches_robs_comment() {
    detail_matches_comment(
        "try_again",
        QKind::CantMeet,
        json!({"answer": "try_again", "guidance": "Read the greeting from the config file"}),
        json!({"answer": "try_again", "guidance": "Read the greeting from the config file"}),
        "Try again with guidance:\n\n    Read the greeting from the config file",
    );
}

#[test]
fn a_changed_request_matches_robs_comment() {
    detail_matches_comment(
        "change_request",
        QKind::CantMeet,
        json!({"answer": "change_request", "text": "Return goodbye instead"}),
        json!({"answer": "change_request", "new_text": "Return goodbye instead"}),
        "Change the request:\n\n    Return goodbye instead",
    );
}

#[test]
fn added_credit_matches_robs_comment() {
    detail_matches_comment(
        "add_credit",
        QKind::OutOfCredit,
        json!({"answer": "add_credit", "amount": 3}),
        json!({"answer": "add_credit", "amount": 3}),
        "Add credit (3).",
    );
}

/// Details Rob must give, and a recommendation only Rob can complete, are
/// refused with the reason and change nothing.
#[test]
fn answers_missing_their_details_are_refused() {
    let mut db = support::Database::start();
    let mut world = World::new(&mut db, "missing_details", &[12, 13], false);
    let cant_meet = world.ask(0, QKind::CantMeet);
    let split = world.ask(1, QKind::NeedsSplitting);
    let cases = [
        (
            0,
            json!({"kind": "answer", "question": cant_meet, "choice": {"answer": "try_again", "guidance": "  "}}),
            "This option needs your words.".to_string(),
        ),
        (
            0,
            json!({"kind": "answer", "question": cant_meet, "choice": {"answer": "add_credit", "amount": 3}}),
            format!("Question {cant_meet} doesn't offer that option."),
        ),
        (
            1,
            json!({"kind": "answer", "question": split, "choice": {"answer": "change_request", "text": ""}}),
            "This option needs your words.".to_string(),
        ),
        (
            1,
            json!({"kind": "defer", "question": split}),
            "TL3's recommended option, \"Change the request\", needs your new request from you."
                .to_string(),
        ),
        (
            0,
            json!({"kind": "defer", "question": cant_meet}),
            "TL3's recommended option, \"Try again with guidance\", needs your guidance from you."
                .to_string(),
        ),
    ];
    for (index, (node, action, reason)) in cases.into_iter().enumerate() {
        let before = world.controller.world().unwrap();
        let replacements = world.rows("replacement");
        let notes = world.notes().len();
        let id = uuid(2200 + index as u32);
        assert!(world.console(&id, node, action.clone()));
        assert_eq!(
            world.outcome(&id),
            ("refused".into(), Some(reason)),
            "{action}"
        );
        assert_eq!(world.controller.world().unwrap(), before, "{action}");
        assert_eq!(world.rows("replacement"), replacements, "{action}");
        assert_eq!(world.notes().len(), notes, "{action}");
    }
    db.stop();
}

/// tl3d drains on every wake its LISTEN session sends: once when the session
/// opens, and again after the session is lost and reconnects.
#[test]
fn wakes_again_after_postgres_restarts() {
    use std::{sync::mpsc, time::Duration};
    use tl3_controller::ingress::Source;
    let mut db = support::Database::start();
    let settings = db.create("wake_reconnect");
    Store::open(&settings).unwrap();
    let (sender, wakes) = mpsc::channel();
    let source = tl3_controller::console::Wakes::new(settings.clone());
    std::thread::spawn(move || source.run(sender));
    let wake = |what: &str| {
        wakes
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("no wake {what}"))
    };
    assert!(matches!(
        wake("when the session opens"),
        tl3_controller::cli::Input::Wake
    ));
    // The controller drains, which answers the doorbell; a filed command rings again.
    tl3_controller::console::Wakes::answered();
    assert!(
        ConsoleCommands::open(&settings)
            .unwrap()
            .submit(&uuid(2300), 0, &json!({"kind": "pause"}))
            .unwrap()
    );
    wake("for a filed command");
    while wakes.try_recv().is_ok() {}
    db.restart();
    wake("after Postgres restarts");
    db.stop();
}

#[test]
fn a_changed_request_answers_a_needs_splitting_question() {
    detail_matches_comment(
        "needs_splitting",
        QKind::NeedsSplitting,
        json!({"answer": "change_request", "text": "Only rename the greeting"}),
        json!({"answer": "change_request", "new_text": "Only rename the greeting"}),
        "Change the request:\n\n    Only rename the greeting",
    );
}

/// The drain answers the doorbell before it reads ingress. A command filed
/// while a drain is under way, after its read, must ring again; answering
/// after the read would swallow that wake and leave the command waiting.
#[test]
fn a_command_filed_during_a_drain_wakes_tl3d_again() {
    use std::{
        sync::{Arc, Mutex, mpsc},
        time::Duration,
    };
    use tl3_controller::ingress::Source;
    let mut db = support::Database::start();
    let world = World::new(&mut db, "drain_order", &[12], false);
    let (sender, wakes) = mpsc::channel();
    std::thread::spawn({
        let source = tl3_controller::console::Wakes::new(world.settings.clone());
        move || source.run(sender)
    });
    let wake = |what: &str| {
        wakes
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|_| panic!("no wake {what}"))
    };
    wake("when the session opens");
    let first = uuid(9000);
    let second = uuid(9001);
    assert!(
        ConsoleCommands::open(&world.settings)
            .unwrap()
            .submit(&first, 0, &json!({"kind": "pause"}))
            .unwrap()
    );
    // The open session's wake covers the first command, so it rings nothing more.
    while wakes.try_recv().is_ok() {}
    // tl3d files the second command from inside the drain, after reading the first.
    let mut store = Store::open(&world.settings).unwrap();
    let filer = Arc::new(Mutex::new(Some(
        ConsoleCommands::open(&world.settings).unwrap(),
    )));
    let consumed = format!("consume-ingress:console:{first}");
    let (resume, resumed) = (filer.clone(), second.clone());
    store.set_boundary_hook(move |point, key, _| {
        if point == SavePoint::ConsumeIngressBeforeCommit
            && key == consumed
            && let Some(mut commands) = resume.lock().unwrap().take()
        {
            assert!(
                commands
                    .submit(&resumed, 0, &json!({"kind": "resume"}))
                    .unwrap()
            );
        }
    });
    let World {
        controller,
        settings,
        root,
        workers,
    } = world;
    drop(controller);
    let mut draining = open(&settings, root.path(), &workers, store);
    tl3_controller::ingress::drain(&mut draining).unwrap();
    assert!(
        filer.lock().unwrap().is_none(),
        "the drain filed the second command"
    );
    wake("for the command filed during the drain");
    db.stop();
}

/// #403: Rob's postmortem button works on closed requests and requests waiting
/// for him, and never files a second postmortem while one is owed or running.
#[test]
fn postmortem_button_files_once_on_closed_or_waiting_requests() {
    let mut db = support::Database::start();
    let mut world = World::new(&mut db, "postmortem_button", &[12], false);
    assert!(world.console(&uuid(1), 0, json!({"kind": "postmortem"})));
    assert_eq!(
        world.outcome(&uuid(1)).0,
        "refused",
        "a running request has no postmortem yet"
    );
    world.console(&uuid(2), 0, json!({"kind": "stop"}));
    assert!(world.node(0).phase.is_closed(), "{:?}", world.node(0).phase);
    let owed = Store::open(&world.settings)
        .unwrap()
        .postmortems("0")
        .unwrap();
    assert_eq!(owed.len(), 1, "{owed:?}");
    assert_eq!(
        (owed[0].trigger.as_str(), owed[0].reason.as_str()),
        ("close", "closed_without_merge")
    );
    world.console(&uuid(3), 0, json!({"kind": "postmortem"}));
    let (status, reason) = world.outcome(&uuid(3));
    assert_eq!(status, "refused");
    assert!(
        reason.unwrap().contains("already"),
        "an owed postmortem is not filed twice"
    );
    raw(&world.settings)
        .execute(
            "UPDATE tl3_postmortems SET status = 'done', finished_at = now()",
            &[],
        )
        .unwrap();
    let notes = world.notes().len();
    world.console(&uuid(4), 0, json!({"kind": "postmortem"}));
    assert_eq!(world.outcome(&uuid(4)).0, "applied");
    let owed = Store::open(&world.settings)
        .unwrap()
        .postmortems("0")
        .unwrap();
    assert_eq!(
        (
            owed[1].trigger.clone(),
            owed[1].reason.as_str(),
            owed[1].status.as_str()
        ),
        (format!("rob:{}", uuid(4)), "rob_asked", "owed")
    );
    assert_eq!(
        world.notes().len(),
        notes,
        "the button posts nothing on the issue"
    );
}
