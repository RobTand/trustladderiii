use serde_json::{Value, json};
use tl3_action::structured::{PAYLOAD_BYTES, Redactor};
#[test]
fn structured_credentials_and_sensitive_file_results_never_publish() {
    let secret = "zE9pR4cV7xB2nM8aQ6tK";
    for input in [
        json!({"api_token":secret}),
        json!({"name":"password", "value":secret}),
        json!({"path":".ssh/id_ed25519", "content":secret}),
        json!({"safe":json!({"password":secret}).to_string()}),
    ] {
        let text = Redactor::default()
            .payload(&input.to_string(), PAYLOAD_BYTES)
            .unwrap();
        assert!(!text.contains(secret), "{text}");
    }
}
#[test]
fn tool_file_provenance_survives_separate_shape_and_clean() {
    let mut redactor = Redactor::default();
    redactor
        .shape(
            &json!({"id":"call-7", "args":{"path":".env"}}).to_string(),
            PAYLOAD_BYTES,
        )
        .unwrap();
    let shaped = redactor
        .shape(
            &json!({"tool_call_id":"call-7", "content":"private file text"}).to_string(),
            PAYLOAD_BYTES,
        )
        .unwrap();
    let safe = redactor.clean(&shaped, PAYLOAD_BYTES).unwrap();
    assert!(!safe.contains("private file text"));
    assert!(safe.contains("sensitive file .env"));
}
#[test]
fn late_loaded_producer_secret_is_redacted_inside_retained_windows() {
    let mut redactor = Redactor::default();
    let secret = "late-capture-7e9d3a4b2c";
    let text = "ordinary line\n".repeat(10_000) + secret;
    let shaped = redactor
        .shape(&json!({"output":text}).to_string(), PAYLOAD_BYTES)
        .unwrap();
    redactor.secrets.insert(secret.into());
    let safe = redactor.clean(&shaped, PAYLOAD_BYTES).unwrap();
    assert!(!safe.contains(secret));
    assert!(safe.contains("[REDACTED]"));
    assert!(safe.len() <= PAYLOAD_BYTES);
}
#[test]
fn every_text_cut_keeps_context_and_never_reveals_a_secret_tail() {
    let secret = "zE9pR4cV7xB2nM8aQ6tK";
    for padding in 0..64 {
        let source = format!("{}password={secret} end", "ordinary ".repeat(padding));
        let mut redactor = Redactor::default();
        redactor.secrets.insert(secret.into());
        for limit in [16, 32, 64, 128] {
            let text = redactor.text(&source, limit).unwrap();
            assert!(!text.contains(secret));
            assert!(!text.contains("B2nM8aQ6tK"));
        }
    }
}
#[test]
fn output_budget_is_measured_after_redaction_with_newest_items_retained() {
    let input = (0..1000)
        .map(|index| json!({"turn":index,"text":"ordinary ".repeat(100)}))
        .collect::<Vec<_>>();
    let mut redactor = Redactor::default();
    let safe = redactor
        .payload(&serde_json::to_string(&input).unwrap(), 4096)
        .unwrap();
    assert!(safe.len() <= 4096);
    let value: Value = serde_json::from_str(&safe).unwrap();
    assert_eq!(value.as_array().unwrap().last().unwrap()["turn"], 999);
    assert!(redactor.truncated);
}
