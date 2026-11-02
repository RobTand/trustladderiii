//! #416: a priority-scaled starting credit reserve, off by default.
#[path = "../../store/tests/support/mod.rs"]
mod support;

use tl3_controller::{Bootstrap, Controller, LabelledIssue, Repository, adapters};
use tl3_store::Store;

fn start_credit(
    settings: &tl3_config::Store,
    title: &str,
    labels: &[&str],
    reserve: &[(&str, u8)],
) -> u32 {
    let root = tempfile::tempdir().unwrap();
    let mut c = Controller::open(
        Store::open(settings).unwrap(),
        adapters::simulation(root.path()).unwrap(),
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
    let mut limits = tl3_config::Config::parse(include_str!("../../config/example.toml"))
        .unwrap()
        .limits;
    limits.priority_credit_reserve = reserve
        .iter()
        .map(|(key, value)| ((*key).to_owned(), *value))
        .collect();
    c.set_priority_credit_reserve(&limits);
    c.intake(LabelledIssue {
        delivery: "label".into(),
        owner: "owner".into(),
        repo: "sandbox".into(),
        installation: 3,
        number: 12,
        title: title.into(),
        actor: 42,
        label: "tl3".into(),
        labels: labels.iter().map(|label| (*label).to_owned()).collect(),
    })
    .unwrap();
    for _ in 0..300 {
        if !c.drive_once().unwrap() {
            break;
        }
    }
    let node = c.world().unwrap().nodes[0];
    assert!(node.credit.ready, "ingest finished");
    node.credit.start
}

#[test]
fn priority_reserve_applies_when_configured_and_changes_nothing_otherwise() {
    let mut db = support::Database::start();
    // The simulated ingest sizes every request at one leaf.
    let size = 1;
    let reserve = [("P0", 5), ("P1", 5)];
    for (name, title, labels, configured, expected) in [
        (
            "p0_title",
            "[P0] Fix the pool",
            &[][..],
            &reserve[..],
            size + 5,
        ),
        (
            "p1_label",
            "Fix the pool",
            &["P1"][..],
            &reserve[..],
            size + 5,
        ),
        (
            "p2_keeps",
            "[P2] Fix the pool",
            &[][..],
            &reserve[..],
            size + 3,
        ),
        ("unranked", "Fix the pool", &[][..], &reserve[..], size + 3),
        ("off_p0", "[P0] Fix the pool", &[][..], &[][..], size + 3),
    ] {
        let settings = db.create(name);
        assert_eq!(
            start_credit(&settings, title, labels, configured),
            expected,
            "{name}"
        );
    }
    db.stop();
}

#[test]
fn an_early_ended_request_keeps_the_kernel_reserve() {
    // The kernel starts an early end's credit at 1 + its reserve of 3, and its
    // IngestEarlyEnd reply carries no size the controller could raise. Applying
    // the priority reserve there needs a kernel change, which #416 excludes; a
    // P0 early end that the audit then turns into a brief keeps the base reserve.
    let mut db = support::Database::start();
    let settings = db.create("early_end");
    let root = tempfile::tempdir().unwrap();
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        tl3_controller::executor::Upstream::Workers,
        EarlyEnd(adapters::workers::Fake::new(root.path())),
    );
    let mut c = Controller::open(
        Store::open(&settings).unwrap(),
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
    let mut limits = tl3_config::Config::parse(include_str!("../../config/example.toml"))
        .unwrap()
        .limits;
    limits.priority_credit_reserve = [("P0".to_owned(), 5)].into();
    c.set_priority_credit_reserve(&limits);
    c.intake(LabelledIssue {
        delivery: "label".into(),
        owner: "owner".into(),
        repo: "sandbox".into(),
        installation: 3,
        number: 12,
        title: "[P0] Already fixed".into(),
        actor: 42,
        label: "tl3".into(),
        labels: Vec::new(),
    })
    .unwrap();
    for _ in 0..300 {
        if !c.drive_once().unwrap() {
            break;
        }
    }
    let node = c.world().unwrap().nodes[0];
    assert!(node.credit.ready);
    assert_eq!(node.credit.start, 1 + tl3_kernel::RESERVE);
    db.stop();
}

/// Ingest proposes an early end instead of a brief.
struct EarlyEnd(adapters::workers::Fake);
impl tl3_controller::executor::Executor for EarlyEnd {
    fn execute(
        &mut self,
        d: &tl3_controller::executor::Delivery,
    ) -> tl3_controller::Result<tl3_controller::executor::Completion> {
        use tl3_controller::executor::{Completion, Observation, Output};
        use tl3_kernel::{EarlyEnd, EffectKind, Event, Reply, Role};
        if matches!(
            d.output,
            Output::Effect {
                kind: EffectKind::StartRun(Role::Ingest),
                ..
            }
        ) {
            return Ok(Completion::confirmed(vec![Observation::Kernel(
                Event::Reply {
                    node: d.node(),
                    generation: d.generation,
                    reply: Reply::IngestEarlyEnd(EarlyEnd::AlreadyDone),
                },
            )]));
        }
        self.0.execute(d)
    }
    fn reconcile(
        &mut self,
        d: &tl3_controller::executor::Delivery,
    ) -> tl3_controller::Result<tl3_controller::executor::Reconciliation> {
        self.0.reconcile(d)
    }
}
