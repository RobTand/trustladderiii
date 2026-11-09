use super::*;

fn cited(test: Value) -> Value {
    activity_section(&json!({"tests": [test]}))["tests"][0].clone()
}

/// PB-REMOVAL M1: a native worker wait reference is a public identifier and must
/// survive memory evidence, so a later reply, operator or postmortem can cite it.
#[test]
fn native_wait_reference_survives_memory_activity() {
    let reference = format!("worker:{}", "a".repeat(64));
    assert_eq!(
        cited(json!({"wait_reference": reference}))["wait_reference"],
        json!(reference)
    );
}

/// Anything other than exactly `worker:` + 64 lowercase hex gets ordinary redaction,
/// and the retired PrismaBuild action_key gets no exemption (no compat).
#[test]
fn malformed_wait_references_and_retired_action_keys_are_redacted() {
    for value in [
        format!("worker:{}", "A".repeat(64)),
        format!("worker:{}", "a".repeat(63)),
        format!("worker:{}", "a".repeat(65)),
        format!("flux:{}", "a".repeat(64)),
        "a".repeat(64),
        format!("worker:{} ghp_fixture1234567890", "a".repeat(64)),
    ] {
        let stored = cited(json!({"wait_reference": value}));
        assert_eq!(
            stored["wait_reference"],
            json!(safe_text(&value)),
            "{value}"
        );
        assert_ne!(stored["wait_reference"], json!(value), "{value}");
    }
    assert_eq!(
        cited(json!({"action_key": "a".repeat(64)}))["action_key"],
        json!("[redacted]")
    );
}
