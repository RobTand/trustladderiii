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
