use serde_json::json;
use tl3_store::WorkerRunFacts;

#[test]
fn stdout_fallback_diagnostic_survives_completion_facts() {
    let value = json!({
        "started_ms": 1, "finished_ms": 2, "outcome": "succeeded", "model": "fixture",
        "prismabuild": [{"phase": "prismabuild_capability", "cause": "machine_record_unsupported",
            "fallback_used": true, "transport": "stdout", "runtime": "pbrun"}]
    });
    let facts = serde_json::from_value::<WorkerRunFacts>(value.clone());
    assert!(
        facts.is_ok(),
        "fallback diagnostic must be accepted: {facts:?}"
    );
    let facts = facts.unwrap();
    facts.validate().unwrap();
    assert_eq!(
        serde_json::to_value(facts).unwrap()["prismabuild"],
        value["prismabuild"]
    );
}

#[test]
fn old_completion_facts_still_validate() {
    let facts: WorkerRunFacts = serde_json::from_value(json!({
        "started_ms": 1, "finished_ms": 2, "outcome": "succeeded", "model": "fixture"
    }))
    .unwrap();
    facts.validate().unwrap();
    assert!(
        serde_json::to_value(facts)
            .unwrap()
            .get("prismabuild")
            .is_none()
    );
}

#[test]
fn fallback_diagnostics_reject_untrusted_or_inconsistent_fields() {
    let value = json!({
        "started_ms": 1, "finished_ms": 2, "outcome": "succeeded", "model": "fixture",
        "prismabuild": [{"phase": "prismabuild_capability", "cause": "machine_record_unsupported",
            "fallback_used": true, "transport": "stdout", "runtime": "pbwait"}]
    });
    for (field, invalid) in [
        ("phase", json!("untrusted")),
        ("cause", json!("fixture-secret")),
        ("runtime", json!("/private/pbwait.py")),
        ("transport", json!("private_fd")),
        ("fallback_used", json!(false)),
    ] {
        let mut changed = value.clone();
        changed["prismabuild"][0][field] = invalid;
        let facts: WorkerRunFacts = serde_json::from_value(changed).unwrap();
        assert!(facts.validate().is_err(), "accepted invalid {field}");
    }
    let mut oversized = value.clone();
    oversized["prismabuild"] = json!(vec![value["prismabuild"][0].clone(); 3]);
    let facts: WorkerRunFacts = serde_json::from_value(oversized).unwrap();
    assert!(facts.validate().is_err());
}
