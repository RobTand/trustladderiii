#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::json;
use std::{
    cell::Cell,
    path::Path,
    rc::Rc,
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tl3_controller::{
    Bootstrap, Controller, Error, LabelledIssue, Repository, adapters,
    cli::Input,
    clock::Clock,
    executor::{Completion, Delivery, Executor, Look, Observation, Reconciliation, Upstream},
};
use tl3_kernel::{Event, NodeId};
use tl3_store::{Commit, RowChange, Store};

#[derive(Clone)]
struct TestClock(Rc<Cell<u64>>);
impl Clock for TestClock {
    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(self.0.get())
    }
}
fn boot() -> Bootstrap {
    Bootstrap {
        repos: vec![Repository {
            owner: "owner".into(),
            name: "sandbox".into(),
            installation: 3,
        }],
        rob: 42,
        slots: 1,
        pool: 1,
        fail_fast: false,
        operations: None,
    }
}
fn issue() -> LabelledIssue {
    LabelledIssue {
        delivery: "label".into(),
        owner: "owner".into(),
        repo: "sandbox".into(),
        installation: 3,
        number: 12,
        title: "Fix greeting".into(),
        actor: 42,
        label: "tl3".into(),
        labels: Vec::new(),
    }
}
fn sql(settings: &tl3_config::Store, statement: &str) {
    postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap()
        .batch_execute(statement)
        .unwrap();
}
fn drain(c: &mut Controller) {
    for _ in 0..200 {
        if !c.drive_once().unwrap() {
            return;
        }
    }
    panic!("controller did not settle");
}
fn run(c: &mut Controller, input: Input) -> tl3_controller::Result<()> {
    let (tx, rx) = mpsc::channel();
    tx.send(input).unwrap();
    drop(tx);
    tl3_controller::ingress::run(c, rx)
}
fn open(settings: &tl3_config::Store, root: &Path) -> Controller {
    Controller::open(
        Store::open(settings).unwrap(),
        adapters::simulation(root).unwrap(),
        boot(),
    )
    .unwrap()
}
fn reject_interns(settings: &tl3_config::Store) {
    sql(
        settings,
        "CREATE FUNCTION reject_intern() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected store failure'; END $$; CREATE TRIGGER reject_intern BEFORE INSERT ON tl3_intern FOR EACH ROW EXECUTE FUNCTION reject_intern();",
    );
}
#[test]
fn intake_store_failure_stops_ingress() {
    let mut db = support::Database::start();
    let settings = db.create("intake_store");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    reject_interns(&settings);
    assert!(
        matches!(
            run(&mut c, Input::LabelledIssue(issue())),
            Err(Error::Store(_))
        ),
        "intake store failure must stop ingress"
    );
}
#[test]
fn observation_store_failure_stops_ingress() {
    let mut db = support::Database::start();
    let settings = db.create("observation_store");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    reject_interns(&settings);
    assert!(
        matches!(
            run(
                &mut c,
                Input::Observation {
                    key: "made".into(),
                    observation: Observation::Made {
                        node: NodeId(0),
                        generation: 1,
                        sha: "a".repeat(40)
                    }
                }
            ),
            Err(Error::Store(_))
        ),
        "observation store failure must stop ingress"
    );
}

/// #771: raw kernel commands cannot bypass steering's command-ID allocation.
#[test]
fn raw_max_command_id_is_rejected_without_changing_the_node() {
    let mut db = support::Database::start();
    let settings = db.create("raw_command_id");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    c.intake(issue()).unwrap();
    let input: Input = serde_json::from_value(json!({
        "Observation": {
            "key": "command-id-jump",
            "observation": {"Kernel": {"Command": {
                "node": 0,
                "id": u64::MAX,
                "command": "Pause"
            }}}
        }
    }))
    .unwrap();
    tl3_controller::ingress::apply(&mut c, input).unwrap();
    drain(&mut c);
    let n = c.world().unwrap().nodes[0];
    assert_eq!(n.last_cmd, 0, "raw input must not assign a command ID");
    assert!(!n.paused, "raw input must not pause the request");
    assert!(c.snapshot().unwrap().rows.iter().any(|row| {
        row.table == "incidents"
            && row.value["input_failure"]["scope"]
                .as_str()
                .is_some_and(|scope| scope.starts_with("observation:"))
    }));
}

fn seed_node_counter(c: &mut Controller, settings: &tl3_config::Store, field: &str, value: u64) {
    let snapshot = c.snapshot().unwrap();
    let mut node = snapshot
        .rows
        .iter()
        .find(|row| row.table == "nodes" && row.id == "0")
        .unwrap()
        .value
        .clone();
    node[field] = json!(value);
    let mut commit = Commit::new(format!("seed-{field}-{value}"), json!(null), &snapshot);
    commit.rows.push(RowChange::Put {
        table: "nodes".into(),
        id: "0".into(),
        value: node,
    });
    Store::open(settings).unwrap().commit(&commit).unwrap();
}

fn counter_incidents(c: &mut Controller) -> Vec<tl3_store::WorldRow> {
    c.snapshot()
        .unwrap()
        .rows
        .into_iter()
        .filter(|r| {
            r.table == "incidents" && r.value["signature"]["class"] == "kernel_counter_headroom"
        })
        .collect()
}

fn clear_counter(c: &mut Controller, id: &str, delivery: &str) {
    c.observe(
        delivery,
        Observation::GitHubComment {
            repository: boot().repos.remove(0),
            issue: 12,
            comment: tl3_github::Comment {
                id: 901,
                body: format!("clear incident {id}"),
                user: tl3_github::Actor {
                    id: 42,
                    login: "rob".into(),
                },
                created_at: "2026-11-05T00:00:00Z".parse().unwrap(),
                updated_at: "2026-11-05T00:00:00Z".parse().unwrap(),
                issue_url: "https://api.github.com/repos/owner/sandbox/issues/12".into(),
            },
        },
    )
    .unwrap();
}

/// State-table "Counter headroom" / Q39: complete edits survive the halt,
/// including a return to the unapplied world's priority, and restart.
#[test]
fn halted_issue_edits_replay_a_priority_reversal_after_restart() {
    let mut db = support::Database::start();
    let settings = db.create("counter_priority_reversal");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    let mut labelled = issue();
    labelled.labels = vec!["P1".into()];
    c.intake(labelled).unwrap();
    let next_seq = c.world().unwrap().nodes[0].next_seq;
    seed_node_counter(&mut c, &settings, "next_seq", u64::MAX);
    for (delivery, priority, at) in [
        ("priority-down", "P2", "2026-11-05T01:00:00Z"),
        ("priority-back", "P1", "2026-11-05T01:00:01Z"),
    ] {
        c.issue_changed(&tl3_controller::IssueChange {
            delivery: delivery.into(),
            owner: "owner".into(),
            repo: "sandbox".into(),
            installation: 3,
            number: 12,
            title: "Fix greeting".into(),
            labels: vec![priority.into()],
            updated_at: at.parse().unwrap(),
        })
        .unwrap();
    }
    assert_eq!(
        c.world().unwrap().nodes[0].priority,
        tl3_kernel::Priority::P1
    );
    let incident = counter_incidents(&mut c)[0].id.clone();
    drop(c);
    let mut c = open(&settings, root.path());
    seed_node_counter(&mut c, &settings, "next_seq", next_seq);
    clear_counter(&mut c, &incident, "clear-priority");
    drain(&mut c);
    assert_eq!(
        c.world().unwrap().nodes[0].priority,
        tl3_kernel::Priority::P1,
        "the later P1 edit must not disappear against the halted world's P1"
    );
}

/// State-table "Counter headroom": premature clearance reopens one incident,
/// then waits for evidence without consuming the retained fact or polling.
#[test]
fn clearing_while_exhausted_reopens_once_then_waits_for_evidence() {
    let mut db = support::Database::start();
    let settings = db.create("counter_premature_clear");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    c.intake(issue()).unwrap();
    seed_node_counter(&mut c, &settings, "next_seq", u64::MAX);
    c.observe(
        "held-fact",
        Observation::Kernel(Event::Fact {
            node: NodeId(0),
            fact: tl3_kernel::Fact::Priority(tl3_kernel::Priority::P1),
        }),
    )
    .unwrap();
    let before = c.world().unwrap();
    assert!(c.drive_once().unwrap()); // Raise the incident.
    assert!(c.drive_once().unwrap()); // Post it once.
    let incident = counter_incidents(&mut c)[0].id.clone();
    clear_counter(&mut c, &incident, "premature-clear");
    assert!(c.drive_once().unwrap()); // Authorized clearance.
    assert!(
        counter_incidents(&mut c)
            .iter()
            .all(|r| r.value["open"] == false)
    );
    assert!(c.drive_once().unwrap()); // Reopen because the counter is still full.
    assert!(c.drive_once().unwrap()); // Post the new incident once.
    let incidents = counter_incidents(&mut c);
    assert_eq!(incidents.len(), 2);
    assert_eq!(
        incidents.iter().filter(|r| r.value["open"] == true).count(),
        1
    );
    let revision = c.snapshot().unwrap().revision;
    assert!(!c.drive_once().unwrap());
    assert!(!c.drive_once().unwrap());
    assert_eq!(
        c.snapshot().unwrap().revision,
        revision,
        "waiting must not commit or poll"
    );
    assert_eq!(c.world().unwrap(), before);
    assert!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "inbox" && r.id == "external:held-fact")
    );
}

/// State-table "Counter headroom" / progress credit: a valid u64 ledger can
/// have a balance above u64::MAX; the actual close records it without panic.
#[test]
fn close_records_a_balance_above_json_integer_range() {
    let mut db = support::Database::start();
    let settings = db.create("counter_wide_close");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    c.intake(issue()).unwrap();
    let snapshot = c.snapshot().unwrap();
    let mut node = snapshot
        .rows
        .iter()
        .find(|r| r.table == "nodes" && r.id == "0")
        .unwrap()
        .value
        .clone();
    node["credit"]["ready"] = json!(true);
    node["credit"]["start"] = json!(258);
    node["credit"]["added"] = json!(u64::MAX - 255);
    let mut commit = Commit::new("seed-wide-credit", json!(null), &snapshot);
    commit.rows.push(RowChange::Put {
        table: "nodes".into(),
        id: "0".into(),
        value: node,
    });
    Store::open(&settings).unwrap().commit(&commit).unwrap();
    let world = c.world().unwrap();
    assert!(world.headroom_ok());
    assert_eq!(world.nodes[0].credit.balance(), u64::MAX as i128 + 3);
    c.observe(
        "stop-wide",
        Observation::Steering {
            node: NodeId(0),
            command: tl3_kernel::Command::Stop,
        },
    )
    .unwrap();
    drain(&mut c);
    assert_eq!(
        c.world().unwrap().nodes[0].phase,
        tl3_kernel::Phase::Closed(tl3_kernel::Outcome::Stopped)
    );
    let grades = Store::open(&settings).unwrap().code_grades("0").unwrap();
    let credit = grades.iter().find(|g| g.dimension == "credit").unwrap();
    assert_eq!(
        credit.fact["balance"],
        json!((u64::MAX as i128 + 3).to_string())
    );
}

/// State-table "Counter headroom": ID exhaustion raises an incident without application.
#[test]
fn exhausted_steering_ids_raise_an_incident_without_an_event() {
    let mut db = support::Database::start();
    let settings = db.create("steering_id_headroom");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    c.intake(issue()).unwrap();
    seed_node_counter(&mut c, &settings, "last_cmd", u64::MAX);
    let before = c.world().unwrap();
    c.observe(
        "pause",
        Observation::Steering {
            node: NodeId(0),
            command: tl3_kernel::Command::Pause,
        },
    )
    .unwrap();
    assert!(c.drive_once().unwrap());
    assert_eq!(c.world().unwrap(), before);
    assert!(c.snapshot().unwrap().rows.iter().any(|row| {
        row.table == "incidents"
            && row.value["summary"] == "command ids exhausted for request 0"
            && row.value["cause"] == "tl3"
    }));
}

/// State-table "Counter headroom": the apply boundary stops before overflow.
#[test]
fn exhausted_kernel_headroom_prevents_the_step_and_reports_the_counter() {
    let mut db = support::Database::start();
    let settings = db.create("kernel_counter_headroom");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    c.intake(issue()).unwrap();
    seed_node_counter(&mut c, &settings, "next_seq", u64::MAX);
    let before = c.world().unwrap();
    c.observe(
        "pause",
        Observation::Steering {
            node: NodeId(0),
            command: tl3_kernel::Command::Pause,
        },
    )
    .unwrap();
    assert!(c.drive_once().unwrap());
    assert_eq!(c.world().unwrap(), before);
    assert!(c.snapshot().unwrap().rows.iter().any(|row| {
        row.table == "incidents"
            && row.value["summary"] == "kernel counter headroom exhausted: next_seq"
    }));
}

/// State-table "Counter headroom" / facts are never dropped. Keep the
/// original arrival order through exhaustion, restart, repair, and Rob's clear.
#[test]
fn exhausted_inputs_remain_queued_until_clearance_then_retry_in_order() {
    let mut db = support::Database::start();
    let settings = db.create("counter_inbox_retention");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    c.intake(issue()).unwrap();
    let next_seq = c.world().unwrap().nodes[0].next_seq;
    seed_node_counter(&mut c, &settings, "next_seq", u64::MAX);
    for (key, priority) in [
        ("first", tl3_kernel::Priority::P1),
        ("second", tl3_kernel::Priority::P2),
    ] {
        c.observe(
            key,
            Observation::Kernel(Event::Fact {
                node: NodeId(0),
                fact: tl3_kernel::Fact::Priority(priority),
            }),
        )
        .unwrap();
    }
    assert!(c.drive_once().unwrap());
    let pending = |c: &mut Controller| {
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.table == "inbox")
            .map(|r| r.id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        pending(&mut c),
        ["external:first", "external:second"],
        "exhaustion must retain facts"
    );
    let incidents = |c: &mut Controller| {
        c.snapshot()
            .unwrap()
            .rows
            .into_iter()
            .filter(|r| {
                r.table == "incidents" && r.value["signature"]["class"] == "kernel_counter_headroom"
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(incidents(&mut c).len(), 1);
    let incident = incidents(&mut c)[0].id.clone();
    assert_eq!(
        incidents(&mut c)[0].value["summary"],
        "kernel counter headroom exhausted: next_seq"
    );
    let mut later_issue = issue();
    later_issue.delivery = "later-label".into();
    later_issue.number = 13;
    assert!(c.intake(later_issue).unwrap());
    assert_eq!(
        c.world().unwrap().nodes.len(),
        1,
        "intake must join the halt queue"
    );
    assert_eq!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.table == "requests")
            .count(),
        1
    );
    drop(c);
    let mut c = open(&settings, root.path());
    drain(&mut c);
    assert_eq!(pending(&mut c), ["external:first", "external:second"]);
    assert_eq!(
        incidents(&mut c).len(),
        1,
        "wakes and restart must deduplicate the incident"
    );
    // Clearance cannot create numeric headroom. Model an operator repair first.
    seed_node_counter(&mut c, &settings, "next_seq", next_seq);
    drain(&mut c);
    assert_eq!(
        c.world().unwrap().nodes[0].priority,
        tl3_kernel::Priority::Unranked,
        "repair alone must not release the halt"
    );
    c.observe(
        "clear-counter",
        Observation::GitHubComment {
            repository: boot().repos.remove(0),
            issue: 12,
            comment: tl3_github::Comment {
                id: 900,
                body: format!("clear incident {incident}"),
                user: tl3_github::Actor {
                    id: 42,
                    login: "rob".into(),
                },
                created_at: "2026-11-05T00:00:00Z".parse().unwrap(),
                updated_at: "2026-11-05T00:00:00Z".parse().unwrap(),
                issue_url: "https://api.github.com/repos/owner/sandbox/issues/12".into(),
            },
        },
    )
    .unwrap();
    for _ in 0..10 {
        c.drive_once().unwrap();
        if !pending(&mut c).contains(&"external:clear-counter".into()) {
            break;
        }
    }
    assert!(!incidents(&mut c)[0].value["open"].as_bool().unwrap());
    assert!(c.drive_once().unwrap());
    assert_eq!(
        c.world().unwrap().nodes[0].priority,
        tl3_kernel::Priority::P1
    );
    assert_eq!(pending(&mut c), ["external:second"]);
    assert!(c.drive_once().unwrap());
    assert_eq!(
        c.world().unwrap().nodes[0].priority,
        tl3_kernel::Priority::P2
    );
    assert!(pending(&mut c).is_empty());
    assert!(c.drive_once().unwrap());
    assert_eq!(c.world().unwrap().nodes.len(), 2);
    assert!(
        !c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "pending_kernel_events")
    );
}

/// Q51 / #771: the new counter halt holds kernel operations, so a new release
/// clears it once through the same path as Rob's authorized clearance.
#[test]
fn new_release_clears_counter_halt_and_retries_retained_fact() {
    let mut db = support::Database::start();
    let settings = db.create("counter_release_clear");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    c.intake(issue()).unwrap();
    let next_seq = c.world().unwrap().nodes[0].next_seq;
    seed_node_counter(&mut c, &settings, "next_seq", u64::MAX);
    c.observe(
        "held-priority",
        Observation::Kernel(Event::Fact {
            node: NodeId(0),
            fact: tl3_kernel::Fact::Priority(tl3_kernel::Priority::P1),
        }),
    )
    .unwrap();
    assert!(c.drive_once().unwrap());
    seed_node_counter(&mut c, &settings, "next_seq", next_seq);
    c.start_release("counter-repair").unwrap();
    assert!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.table == "incidents")
            .all(|r| r.value["open"] == false),
        "Q51 must clear the counter halt"
    );
    assert!(c.drive_once().unwrap());
    assert_eq!(
        c.world().unwrap().nodes[0].priority,
        tl3_kernel::Priority::P1
    );
    assert!(
        !c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "inbox")
    );
    c.start_release("counter-repair").unwrap();
    assert_eq!(
        c.world().unwrap().nodes[0].priority,
        tl3_kernel::Priority::P1
    );
}

/// #771: widening the serialized ledger reads old JSON unchanged, and the
/// derived balance must contain the sum of all four full-width entries.
#[test]
fn serialized_world_accepts_old_and_full_width_credit_ledgers() {
    let mut world = tl3_kernel::World::new(&[1], 1);
    world
        .nodes
        .push(tl3_kernel::Node::new(tl3_kernel::RepoId(0)));
    let mut encoded = serde_json::to_value(&world).unwrap();
    for value in [u32::MAX as u64, u64::MAX] {
        for field in ["start", "earned", "added", "spent"] {
            encoded["nodes"][0]["credit"][field] = json!(value);
        }
        let restored: tl3_kernel::World = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), encoded);
        assert_eq!(
            restored.nodes[0].credit.balance().to_string(),
            (2 * value as i128).to_string()
        );
    }
}
struct BrokenLook {
    fake: adapters::workers::Fake,
    settings: tl3_config::Store,
    corruption: bool,
    calls: Rc<Cell<usize>>,
}
impl Executor for BrokenLook {
    fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
        Err(std::io::Error::from(std::io::ErrorKind::ConnectionReset).into())
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(Reconciliation::Pending)
    }
    fn look(&mut self, d: &Delivery) -> tl3_controller::Result<Look> {
        self.calls.set(self.calls.get() + 1);
        if !self.corruption {
            return Err(Error::Store(tl3_store::Error::Invalid(
                "injected store failure",
            )));
        }
        // Corrupt the disposable DB after the outer look has hydrated its snapshot.
        sql(
            &self.settings,
            &format!(
                "ALTER TABLE tl3_effects DISABLE TRIGGER tl3_effect_guard; UPDATE tl3_effects SET payload = '{{}}' WHERE effect_key = '{}'; ALTER TABLE tl3_effects ENABLE TRIGGER tl3_effect_guard;",
                d.key
            ),
        );
        Ok(Look::Found(Box::new(Completion::confirmed(vec![]))))
    }
    fn retire_delivery(&mut self, d: &Delivery) -> tl3_controller::Result<()> {
        self.fake.retire_delivery(d)
    }
}
fn broken_look(corruption: bool) {
    let mut db = support::Database::start();
    let settings = db.create("broken_look");
    let root = tempfile::tempdir().unwrap();
    let calls = Rc::new(Cell::new(0));
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Workers,
        BrokenLook {
            fake: adapters::workers::Fake::new(root.path()),
            settings: settings.clone(),
            corruption,
            calls: calls.clone(),
        },
    );
    let mut c = Controller::open(Store::open(&settings).unwrap(), registry, boot()).unwrap();
    c.intake(issue()).unwrap();
    drain(&mut c);
    let result = run(&mut c, Input::Wake);
    assert!(
        calls.get() > 0,
        "injection must run inside the look boundary"
    );
    if corruption {
        assert!(
            result.is_err(),
            "corrupt stored effect payload must stop ingress instead of opening a setup incident"
        );
        assert!(
            matches!(result, Err(Error::StoredJson(_))),
            "stored effect decoding must have a distinct fatal error"
        );
        assert!(
            !c.snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "incidents"),
            "stored corruption must not become an incident"
        );
    } else {
        assert!(
            matches!(result, Err(Error::Store(_))),
            "look store failure must stop ingress"
        );
    }
}
#[test]
fn corrupt_completion_payload_stops_ingress() {
    broken_look(true);
}
#[test]
fn look_store_failure_stops_ingress() {
    broken_look(false);
}

#[test]
fn corrupt_stored_inbox_stops_ingress() {
    let mut db = support::Database::start();
    let settings = db.create("corrupt_inbox");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    let mut store = Store::open(&settings).unwrap();
    let snapshot = store.load().unwrap();
    let mut commit = tl3_store::Commit::new("broken-inbox", json!({}), &snapshot);
    commit.rows.push(tl3_store::RowChange::Put {
        table: "inbox".into(),
        id: "broken".into(),
        value: json!({}),
    });
    store.commit(&commit).unwrap();
    let result = run(&mut c, Input::Wake);
    assert!(
        matches!(result, Err(Error::StoredJson(_))),
        "corrupt stored inbox must stop ingress"
    );
    assert!(
        !c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents"),
        "stored corruption must not become an incident"
    );
}

struct RefusedPosts(Rc<Cell<usize>>);
impl Executor for RefusedPosts {
    fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
        unreachable!()
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        unreachable!()
    }
    fn input_incident(
        &mut self,
        _: &tl3_controller::input_errors::IncidentNotice,
        _: i64,
    ) -> tl3_controller::Result<()> {
        self.0.set(self.0.get() + 1);
        Err(tl3_github::Error::Http { status: 403 }.into())
    }
}
#[test]
fn failed_post_backoff_survives_wakes_and_restart() {
    let mut db = support::Database::start();
    let settings = db.create("post_backoff");
    let root = tempfile::tempdir().unwrap();
    let clock = TestClock(Rc::new(Cell::new(1000)));
    let calls = Rc::new(Cell::new(0));
    let reopen = || {
        let mut registry = adapters::simulation(root.path()).unwrap();
        registry.register(Upstream::GitHub, RefusedPosts(calls.clone()));
        let mut bootstrap = boot();
        bootstrap.operations = Some(tl3_config::OperationsIssue {
            owner: "owner".into(),
            name: "operations".into(),
            installation_id: 3,
            number: 17,
        });
        Controller::open_with_clock(
            Store::open(&settings).unwrap(),
            registry,
            bootstrap,
            clock.clone(),
        )
        .unwrap()
    };
    let mut c = reopen();
    c.observe(
        "invalid",
        Observation::Kernel(Event::NewRequest {
            repo: tl3_kernel::RepoId(0),
            priority: tl3_kernel::Priority::Unranked,
        }),
    )
    .unwrap();
    drain(&mut c);
    assert_eq!(calls.get(), 1);
    for _ in 0..3 {
        tl3_controller::ingress::apply(&mut c, Input::GitHubWake).unwrap();
        drain(&mut c);
    }
    assert_eq!(
        calls.get(),
        1,
        "permanently failing posts must back off across GitHub wakes"
    );
    drop(c);
    let mut c = reopen();
    c.look_now().unwrap();
    drain(&mut c);
    assert_eq!(
        calls.get(),
        1,
        "post backoff must survive controller restart"
    );
    clock.0.set(clock.0.get() + 3600);
    tl3_controller::ingress::apply(&mut c, Input::GitHubWake).unwrap();
    drain(&mut c);
    assert_eq!(calls.get(), 2, "a later wake must retry after backoff");
}

#[test]
fn missing_destination_logs() {
    if std::env::var_os("TL3_MISSING_DESTINATION_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "missing_destination_logs", "--nocapture"])
            .env("TL3_MISSING_DESTINATION_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("no issue or operations destination"),
            "unpostable incident needs an explicit destination diagnostic: {stderr}"
        );
        return;
    }
    let mut db = support::Database::start();
    let settings = db.create("missing_destination");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    c.observe(
        "invalid",
        Observation::Kernel(Event::NewRequest {
            repo: tl3_kernel::RepoId(0),
            priority: tl3_kernel::Priority::Unranked,
        }),
    )
    .unwrap();
    drain(&mut c);
    assert_eq!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.table == "incidents")
            .count(),
        1
    );
}

#[test]
fn corrupt_incident_during_probe_stops_ingress() {
    struct CorruptProbe(tl3_config::Store);
    impl Executor for CorruptProbe {
        fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
            Err(std::io::Error::from(std::io::ErrorKind::ConnectionReset).into())
        }
        fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
            Ok(Reconciliation::Pending)
        }
        fn probe(&mut self, _: &Delivery) -> tl3_controller::Result<bool> {
            sql(
                &self.0,
                "UPDATE tl3_world SET value = value - 'signature' WHERE table_name = 'incidents' AND row_id = 'probe-incident'",
            );
            Ok(true)
        }
    }
    let mut db = support::Database::start();
    let settings = db.create("corrupt_probe_incident");
    let root = tempfile::tempdir().unwrap();
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(Upstream::Workers, CorruptProbe(settings.clone()));
    let mut c = Controller::open(Store::open(&settings).unwrap(), registry, boot()).unwrap();
    c.intake(issue()).unwrap();
    drain(&mut c);
    let mut store = Store::open(&settings).unwrap();
    let snapshot = store.load().unwrap();
    let row = snapshot
        .rows
        .iter()
        .find(|r| r.table == "allowances")
        .unwrap();
    let key = row.value["delivery"]["key"].as_str().unwrap();
    let mut value = row.value.clone();
    value["incident"] = json!("probe-incident");
    let mut commit = tl3_store::Commit::new("probe-incident", json!({}), &snapshot);
    commit.rows.push(tl3_store::RowChange::Put {
        table: "allowances".into(),
        id: row.id.clone(),
        value,
    });
    commit.rows.push(tl3_store::RowChange::Put { table: "incidents".into(), id: "probe-incident".into(), value: json!({"id":"probe-incident", "node":0, "open":true, "needs_rob":false, "blocking":false, "summary":"read failed", "signature":{"upstream":"Workers", "class":"transport"}, "effect":key}) });
    store.commit(&commit).unwrap();
    let result = run(&mut c, Input::Wake);
    assert!(
        result.is_err(),
        "incident corruption read inside a probe must stop ingress"
    );
    assert!(
        matches!(result, Err(Error::StoredJson(_))),
        "stored incident decoding must have a distinct fatal error"
    );
    assert!(
        !c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "allowances" && r.value["failure"] == "setup"),
        "corruption must stop before mutating the allowance into a setup failure"
    );
}

#[test]
fn stored_repository_in_translator_has_fatal_decode_error() {
    use tl3_controller::ingress::{GitHubDelivery, GitHubTranslator};
    let mut db = support::Database::start();
    let settings = db.create("translator_repository");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path());
    sql(
        &settings,
        "UPDATE tl3_world SET value = value - 'name' WHERE table_name = 'repos'",
    );
    let snapshot = c.snapshot().unwrap();
    let mut translator = adapters::mergify::Translator::new(Default::default()).unwrap();
    let delivery = GitHubDelivery::CatchUp {
        repository: boot().repos.remove(0),
        changes: Box::new(tl3_github::RepoChanges {
            repository: "owner/sandbox".into(),
            issues: vec![],
            comments: vec![],
            labels: vec![],
            closed: vec![],
            pull_requests: vec![],
            check_runs: vec![],
            check_suites: vec![],
        }),
    };
    let result = translator.translate(&delivery, &snapshot);
    assert!(result.is_err());
    assert!(
        matches!(result, Err(Error::StoredJson(_))),
        "stored repository decoding inside a translator must use a fatal error"
    );
}
