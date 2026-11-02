//! Q50 environment blockers and Q51 release clears (Rob, 2026-11-01).
#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::{Value, json};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    executor::{
        Completion, Delivery, Executor, Look, Observation, Output, Reconciliation, Upstream,
    },
};
use tl3_kernel::{EffectKind, Event, NodeId, Phase, Reply, Role};
use tl3_store::Store;

const KEY: &str = "c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3";

fn starts(d: &Delivery, effect: EffectKind) -> bool {
    matches!(d.output, Output::Effect { kind, .. } if kind == effect)
}

/// Holds maker runs open, so each scenario supplies the maker's reply. An
/// unreadable maker fails every look, as an unreadable run directory would.
struct HeldMaker(adapters::workers::Fake, bool);
impl Executor for HeldMaker {
    fn look(&mut self, d: &Delivery) -> tl3_controller::Result<Look> {
        if self.1 && starts(d, EffectKind::StartRun(Role::Make)) {
            return Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into());
        }
        Ok(match self.reconcile(d)? {
            Reconciliation::Found(completion) => Look::Found(completion),
            Reconciliation::Absent => Look::Absent,
            Reconciliation::Pending => Look::Pending,
        })
    }
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        if starts(d, EffectKind::StartRun(Role::Make)) {
            Ok(Completion::confirmed(vec![]))
        } else {
            self.0.execute(d)
        }
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        if starts(d, EffectKind::StartRun(Role::Make)) {
            Ok(Reconciliation::Pending)
        } else {
            self.0.reconcile(d)
        }
    }
}

/// PrismaBuild loses every check job: an upstream infrastructure failure.
struct LostChecks(adapters::prismabuild::Fake);
impl Executor for LostChecks {
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        if starts(d, EffectKind::SubmitChecks) {
            Ok(Completion::confirmed(vec![Observation::Kernel(
                Event::Reply {
                    node: d.node(),
                    generation: d.generation,
                    reply: Reply::RunFailed,
                },
            )]))
        } else {
            self.0.execute(d)
        }
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.0.reconcile(d)
    }
}

fn open(settings: &tl3_config::Store, root: &std::path::Path, upstream: Upstream) -> Controller {
    open_with(settings, root, upstream, Some("release-1"), false)
}

fn open_with(
    settings: &tl3_config::Store,
    root: &std::path::Path,
    upstream: Upstream,
    release: Option<&str>,
    unreadable: bool,
) -> Controller {
    let mut registry = adapters::simulation(root).unwrap();
    match upstream {
        Upstream::Workers => registry.register(
            upstream,
            HeldMaker(adapters::workers::Fake::new(root), unreadable),
        ),
        Upstream::PrismaBuild => {
            registry.register(upstream, LostChecks(adapters::prismabuild::Fake::new(root)))
        }
        _ => unreachable!(),
    }
    let mut c = Controller::open(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap {
            repos: vec![Repository {
                owner: "owner".into(),
                name: "sandbox".into(),
                installation: 3,
            }],
            rob: 42,
            slots: 1,
            pool: 1,
            operations: None,
        },
    )
    .unwrap();
    if let Some(release) = release {
        start(&mut c, release);
    }
    c.intake(LabelledIssue {
        delivery: "label".into(),
        owner: "owner".into(),
        repo: "sandbox".into(),
        installation: 3,
        number: 12,
        title: "Fix a greeting".into(),
        actor: 42,
        label: "tl3".into(),
    })
    .unwrap();
    drain(&mut c);
    c
}

fn drain(c: &mut Controller) {
    for _ in 0..300 {
        if !c.drive_once().unwrap() {
            return;
        }
    }
    panic!("controller did not settle");
}

fn start(c: &mut Controller, release: &str) {
    c.start_release(release).unwrap();
    drain(c);
}

fn maker_replies(c: &mut Controller, reply: &Value, count: usize) {
    for _ in 0..count {
        let node = c.world().unwrap().nodes[0];
        assert_eq!(node.phase, Phase::Making);
        let generation = node.run.expect("a held maker run");
        c.observe(
            &format!("maker-reply:{generation}"),
            Observation::WorkerReply {
                node: NodeId(0),
                generation,
                role: "maker".into(),
                reply: reply.clone(),
            },
        )
        .unwrap();
        drain(c);
    }
}

/// The reply as worker code emits it: evidence it checked, with the class it
/// recorded. The report is model text and must never reach GitHub.
fn blocked() -> Value {
    json!({"kind": "blocked",
        "evidence": {"action_key": KEY, "class": "test_infrastructure"},
        "report": "@octocat see REPORT-TEXT for the broken sandbox."})
}

fn open_incidents(c: &mut Controller) -> Vec<Value> {
    c.snapshot()
        .unwrap()
        .rows
        .into_iter()
        .filter(|r| r.table == "incidents" && r.value["open"] == true)
        .map(|r| r.value)
        .collect()
}

#[test]
fn blocked_reply_spends_no_credit_and_cites_evidence_on_second_failure() {
    let mut db = support::Database::start();
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&db.create("blocked_reply"), root.path(), Upstream::Workers);
    let before = c.world().unwrap().nodes[0];
    let generation = before.run.unwrap();

    maker_replies(&mut c, &blocked(), 1);
    let node = c.world().unwrap().nodes[0];
    assert_eq!(
        node.credit, before.credit,
        "a blocked reply spends no credit"
    );
    assert_eq!(node.attempt, before.attempt);
    assert_eq!(node.run_failures, 1);
    assert!(!node.waiting_incident);
    assert!(
        node.run.unwrap() > generation,
        "it retries in a new generation"
    );

    maker_replies(&mut c, &blocked(), 1);
    let node = c.world().unwrap().nodes[0];
    assert_eq!(node.credit, before.credit);
    assert!(node.waiting_incident && !node.incident_needs_rob);
    let incidents = open_incidents(&mut c);
    assert_eq!(incidents.len(), 1);
    let incident = &incidents[0];
    assert_eq!(incident["blocking"], true);
    assert_eq!(incident["cause"], "tl3");
    assert_eq!(
        incident["signature"],
        json!({"upstream": "Workers", "class": "environment_blocked"})
    );
    let summary = incident["summary"].as_str().unwrap();
    assert!(summary.contains(KEY), "{summary}");
    assert!(summary.contains("test infrastructure error"), "{summary}");
    assert!(
        !summary.contains("REPORT-TEXT") && !summary.contains('@'),
        "model text stays off GitHub: {summary}"
    );
    assert!(c.world().unwrap().valid());
    db.stop();
}

#[test]
fn blocked_reply_without_evidence_is_rejected_as_an_invalid_reply() {
    let mut db = support::Database::start();
    let root = tempfile::tempdir().unwrap();
    let mut c = open(
        &db.create("blocked_rejected"),
        root.path(),
        Upstream::Workers,
    );
    let before = c.world().unwrap().nodes[0];
    for reply in [
        json!({"kind": "blocked", "report": "The sandbox refused to run the tests."}),
        json!({"kind": "blocked", "evidence": {"action_key": KEY}, "report": "No class."}),
    ] {
        maker_replies(&mut c, &reply, 1);
    }
    let node = c.world().unwrap().nodes[0];
    assert_eq!(node.credit, before.credit);
    assert!(node.waiting_incident);
    let incidents = open_incidents(&mut c);
    assert_eq!(incidents.len(), 1);
    // A reply that breaks the protocol is still TL3's own failure, but it cites nothing.
    assert_eq!(incidents[0]["cause"], "tl3");
    assert_ne!(incidents[0]["signature"]["class"], "environment_blocked");
    assert!(
        !incidents[0]["summary"]
            .as_str()
            .unwrap()
            .contains("Evidence")
    );
    db.stop();
}

#[test]
fn new_release_clears_tl3_incident_once_and_same_release_restart_clears_nothing() {
    let mut db = support::Database::start();
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&db.create("release_clears"), root.path(), Upstream::Workers);
    maker_replies(&mut c, &blocked(), 2);
    assert!(c.world().unwrap().nodes[0].waiting_incident);

    start(&mut c, "release-1");
    assert!(
        c.world().unwrap().nodes[0].waiting_incident,
        "a restart on the same release clears nothing"
    );
    assert_eq!(open_incidents(&mut c).len(), 1);

    start(&mut c, "release-2");
    let node = c.world().unwrap().nodes[0];
    assert!(
        !node.waiting_incident,
        "a new release clears TL3's incident"
    );
    assert!(open_incidents(&mut c).is_empty());
    assert_eq!(node.phase, Phase::Making, "the operation retries");
    assert!(node.run.is_some());

    // Failing again on the new release reopens the incident, and only Rob or a
    // later release clears it.
    maker_replies(&mut c, &blocked(), 2);
    let node = c.world().unwrap().nodes[0];
    assert!(node.waiting_incident && node.incident_needs_rob);
    assert_eq!(open_incidents(&mut c).len(), 1);
    start(&mut c, "release-2");
    assert!(c.world().unwrap().nodes[0].waiting_incident);

    start(&mut c, "release-3");
    let node = c.world().unwrap().nodes[0];
    assert!(!node.waiting_incident);
    assert_eq!(node.phase, Phase::Making);
    assert!(open_incidents(&mut c).is_empty());
    assert!(c.world().unwrap().valid());
    db.stop();
}

#[test]
fn new_release_leaves_upstream_incidents() {
    let mut db = support::Database::start();
    let root = tempfile::tempdir().unwrap();
    let mut c = open(
        &db.create("release_upstream"),
        root.path(),
        Upstream::PrismaBuild,
    );
    let node = c.world().unwrap().nodes[0];
    assert!(node.waiting_incident, "{:?}", node.phase);
    let incidents = open_incidents(&mut c);
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0]["cause"], "upstream");
    assert_eq!(incidents[0]["signature"]["upstream"], "PrismaBuild");

    start(&mut c, "release-2");
    assert!(c.world().unwrap().nodes[0].waiting_incident);
    assert_eq!(open_incidents(&mut c).len(), 1);
    db.stop();
}

#[test]
fn incident_cites_blocked_evidence_only_when_the_blocked_reply_raised_it() {
    let mut db = support::Database::start();
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&db.create("blocked_window"), root.path(), Upstream::Workers);
    maker_replies(&mut c, &blocked(), 1);
    maker_replies(&mut c, &json!({"kind": "done"}), 1);
    let incidents = open_incidents(&mut c);
    assert_eq!(incidents.len(), 1);
    assert_ne!(incidents[0]["signature"]["class"], "environment_blocked");
    let summary = incidents[0]["summary"].as_str().unwrap();
    assert!(!summary.contains(KEY), "{summary}");
    db.stop();
}

#[test]
fn first_install_without_a_recorded_release_clears_tl3_incidents() {
    let mut db = support::Database::start();
    let root = tempfile::tempdir().unwrap();
    let mut c = open_with(
        &db.create("release_first"),
        root.path(),
        Upstream::Workers,
        None,
        false,
    );
    maker_replies(&mut c, &blocked(), 2);
    assert!(c.world().unwrap().nodes[0].waiting_incident);
    assert!(
        !c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "releases")
    );
    start(&mut c, "release-1");
    assert!(!c.world().unwrap().nodes[0].waiting_incident);
    assert!(open_incidents(&mut c).is_empty());
    maker_replies(&mut c, &blocked(), 2);
    start(&mut c, "release-1");
    assert!(c.world().unwrap().nodes[0].waiting_incident);
    db.stop();
}

#[test]
fn new_release_clears_a_tl3_side_look_incident_that_holds_an_effect() {
    let mut db = support::Database::start();
    let root = tempfile::tempdir().unwrap();
    let mut c = open_with(
        &db.create("release_effect"),
        root.path(),
        Upstream::Workers,
        Some("release-1"),
        true,
    );
    // The running maker's look, which an unreadable run directory fails.
    let key = c
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .filter(|r| r.table == "allowances" && r.id.starts_with("effect:0:"))
        .max_by_key(|r| r.id.rsplit(':').next().unwrap().parse::<u64>().unwrap())
        .unwrap()
        .id;
    c.look_key(&key).unwrap();
    c.look_key(&key).unwrap();
    let incidents = open_incidents(&mut c);
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0]["blocking"], false);
    assert_eq!(incidents[0]["effect"], key);
    assert_eq!(incidents[0]["cause"], "tl3");
    start(&mut c, "release-1");
    assert_eq!(open_incidents(&mut c).len(), 1);
    start(&mut c, "release-2");
    assert!(open_incidents(&mut c).is_empty());
    db.stop();
}

#[test]
fn release_identity_is_the_built_source_tree() {
    let id = tl3_controller::release::id();
    let tree = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "HEAD^{tree}"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8(output.stdout).unwrap().trim().to_owned());
    match tree {
        Some(tree) => assert_eq!(id, tree),
        None => {
            let digest = id
                .strip_prefix("sha256:")
                .expect("a binary digest without Git");
            assert!(digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }
}
