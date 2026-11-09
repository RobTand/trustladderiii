use serde_json::json;
use tl3_store::WorkerRunFacts;

#[test]
fn removed_prismabuild_facts_are_rejected_at_decode() {
    for legacy in [
        json!([{"phase": "prismabuild_capability", "cause": "machine_record_unsupported",
            "fallback_used": true, "transport": "stdout", "runtime": "pbrun"}]),
        json!([]),
    ] {
        let value = json!({
            "started_ms": 1, "finished_ms": 2, "outcome": "succeeded", "model": "fixture",
            "prismabuild": legacy
        });
        let error = serde_json::from_value::<WorkerRunFacts>(value).unwrap_err();
        assert!(
            error.to_string().contains("unknown field `prismabuild`"),
            "active removed field must be refused at decode: {error}"
        );
    }
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
