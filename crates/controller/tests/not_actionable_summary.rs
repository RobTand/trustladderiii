use serde_json::{Value, json};
use tl3_controller::{
    evidence::{capture, summary},
    executor::Delivery,
};
use tl3_store::{Commit, Epoch, Revision, Snapshot, WorldRow};

const REASON: &str =
    "The pinned base does not contain the merge prerequisites that the issue names.";
const MISSING: &str = "The reason for stopping was not captured.";

fn delivery() -> Delivery {
    serde_json::from_value(json!({
        "key":"summary:118", "epoch":1, "generation":7,
        "output":{"Notice":{"node":118,"notice":{"Summary":"NotActionable"}}},
        "repository":{"owner":"RobTand","name":"prismabuild","installation":1},
        "issue":2763,"paths":[],"attempt":1,"round":0
    }))
    .unwrap()
}

fn snapshot(brief: Value) -> Snapshot {
    Snapshot {
        revision: Revision::ZERO,
        epoch: Epoch::ZERO,
        rows: vec![WorldRow {
            arrival: 0,
            version: Revision::ZERO,
            table: "briefs".into(),
            id: "118".into(),
            value: brief,
        }],
        pending_effects: vec![],
        credit: vec![],
        interned: vec![],
    }
}

#[test]
fn summary_states_recorded_ingest_reason() {
    let s = snapshot(json!({"kind":"early_end","outcome":"not_actionable","reason":REASON}));
    let mut d = delivery();
    d.evidence = Some(capture(&s, &Commit::new("close", json!({}), &s), &d).unwrap());
    let text = summary(&d).unwrap();
    assert!(text.contains(REASON), "{text}");
    assert!(!text.contains(MISSING), "{text}");
    assert_eq!(text, summary(&d).unwrap());
}

#[test]
fn summary_uses_pending_ingest_reason() {
    let s = snapshot(json!({"kind":"early_end","outcome":"not_actionable","reason":"Old reason"}));
    let mut c = Commit::new("close", json!({}), &s);
    c.rows.push(tl3_store::RowChange::Put {
        table: "briefs".into(),
        id: "118".into(),
        value: json!({"kind":"early_end","outcome":"not_actionable","reason":REASON}),
    });
    let mut d = delivery();
    d.evidence = Some(capture(&s, &c, &d).unwrap());
    let text = summary(&d).unwrap();
    assert!(text.contains(REASON), "{text}");
    assert!(!text.contains("Old reason"), "{text}");
    assert!(!text.contains(MISSING), "{text}");
}

#[test]
fn summary_redacts_and_quotes_ingest_reason() {
    let s = snapshot(json!({
        "kind":"early_end","outcome":"not_actionable",
        "reason":"Missing prerequisite; password=Z7vQ9mN4pR6xT8cL2hK5sD3f <details> @operator `command`"
    }));
    let mut d = delivery();
    d.evidence = Some(capture(&s, &Commit::new("close", json!({}), &s), &d).unwrap());
    let text = summary(&d).unwrap();
    assert!(text.contains("Missing prerequisite"), "{text}");
    assert!(!text.contains("Z7vQ9mN4pR6xT8cL2hK5sD3f"), "{text}");
    assert!(text.contains("[details] [at]operator 'command'"), "{text}");
    assert!(!text.contains(MISSING), "{text}");
}

#[test]
fn summary_reports_missing_reason_only_without_a_recorded_reason() {
    for brief in [
        json!({"kind":"early_end","outcome":"not_actionable"}),
        json!({"kind":"early_end","outcome":"not_actionable","reason":null}),
    ] {
        let s = snapshot(brief);
        let mut d = delivery();
        d.evidence = Some(capture(&s, &Commit::new("close", json!({}), &s), &d).unwrap());
        let text = summary(&d).unwrap();
        assert!(text.contains(MISSING), "{text}");
    }
}

#[test]
fn summary_does_not_borrow_another_requests_reason() {
    let mut s = snapshot(json!({"kind":"early_end","outcome":"not_actionable","reason":REASON}));
    s.rows[0].id = "119".into();
    let mut d = delivery();
    d.evidence = Some(capture(&s, &Commit::new("close", json!({}), &s), &d).unwrap());
    let text = summary(&d).unwrap();
    assert!(!text.contains(REASON), "{text}");
    assert!(text.contains(MISSING), "{text}");
}
