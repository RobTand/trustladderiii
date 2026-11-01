//! Q50 environment blockers and Q51 release clears (Rob, 2026-11-01).
#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::{Value, json};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    executor::{Completion, Delivery, Executor, Observation, Output, Reconciliation, Upstream},
};
use tl3_kernel::{EffectKind, Event, NodeId, Phase, Reply, Role};
use tl3_store::Store;

const KEY: &str = "c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3";

fn starts(d: &Delivery, effect: EffectKind) -> bool {
    matches!(d.output, Output::Effect { kind, .. } if kind == effect)
}

/// Holds maker runs open, so each scenario supplies the maker's reply.
struct HeldMaker(adapters::workers::Fake);
impl Executor for HeldMaker {
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
    let mut registry = adapters::simulation(root).unwrap();
    match upstream {
        Upstream::Workers => {
            registry.register(upstream, HeldMaker(adapters::workers::Fake::new(root)))
        }
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
    start(&mut c, "release-1");
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

fn blocked() -> Value {
    json!({"kind": "blocked", "evidence": {"action_key": KEY},
        "report": "PrismaBuild returned an infrastructure error for every test run."})
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
    assert!(summary.contains("infrastructure error"), "{summary}");
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
        json!({"kind": "blocked", "evidence": {"action_key": "short"}, "report": "Broken."}),
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
fn release_identity_names_this_build() {
    let id = tl3_controller::release::id();
    let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
    assert!(
        hex(&id, 40) || hex(&id, 64) || id.strip_prefix("sha256:").is_some_and(|h| hex(h, 64)),
        "{id:?}"
    );
}
