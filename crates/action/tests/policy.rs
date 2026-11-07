use tl3_action::{PolicyOperation, PolicyRequest, PolicyResult, policy};
use tl3_redaction::structured::Redactor;
use tl3_contracts::rules::{self, *};
use serde_json::json;
fn row() -> Violation {
    Violation { rule_id: "proof.subject".into(), role: Role::Maker,
        names: vec![Name { kind: NameKind::Path, value: "test_subject.py".into() }],
        bad_value: json!("bad"), allowed: Allowed::Finite { values: vec![] },
        repair: Repair::SourceEdit { path: "test_subject.py".into(), instruction: "Keep the complete source repair.".into() },
        gating: true, evidence: vec![], evidence_truncated: 0 }
}
#[test]
fn producer_secrets_are_removed_before_refusal_publication() {
    let secret = "producer-secret-a7B9c3D8e2F6";
    let row = row().with_evidence(vec![Evidence::TestFailure { target: "//tests:subject".into(), test: "case[é]".into(), excerpt: format!("API_TOKEN={secret}"), log_ref: "log://failure".into(), receipt: "receipt".into() }]);
    let mut state = Redactor::default(); state.secrets.insert(secret.into());
    let response = policy(PolicyRequest { state, operation: PolicyOperation::Render { rows: vec![row], role: Role::Maker, context: RenderContext::Tool } }).unwrap();
    let PolicyResult::Text(text) = response.result else { panic!("expected text") };
    assert!(!text.contains(secret)); assert!(text.contains("[REDACTED]"));
    assert!(text.contains("Keep the complete source repair.")); assert!(text.contains("Further checks were not run."));
}
#[test]
fn final_size_and_context_fail_explicitly() {
    let mut row = row(); row.allowed = Allowed::Finite { values: vec![json!("é".repeat(rules::MESSAGE_BYTES))] };
    assert!(policy(PolicyRequest { state: Redactor::default(), operation: PolicyOperation::Render { rows: vec![row], role: Role::Maker, context: RenderContext::Tool } }).is_err());
}
#[test]
fn expected_red_declaration_is_not_observed_match_evidence() {
    let declaration = json!({"subjects":["a.py::value"], "observer_support":[], "red_message":"the expected message"});
    let mut observed = declaration.clone(); observed["observed_message"] = json!("model authored capture");
    assert!(serde_json::from_value::<tl3_contracts::ProofDeclaration>(declaration).is_ok());
    assert!(serde_json::from_value::<tl3_contracts::ProofDeclaration>(observed).is_err());
}
