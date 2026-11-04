use proptest::prelude::*;
use serde_json::{Value, json};
use std::process::{Command, Stdio};
use tl3_controller::classifier::validate;

fn inputs() -> Value {
    json!({"comment":{"author":"requester","body":"comment"},
        "state":{"closed":false,"phase":"Making","open_question":null}})
}

#[test]
fn python_classifier_golden_cases() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("classifier/golden.json")).unwrap();
    assert!(
        cases.len() >= 90,
        "the old classifier corpus must be retained"
    );
    for case in cases {
        let actual = validate(&case["inputs"], &case["reply"]);
        if case["schema_invalid"] == true {
            assert!(actual.is_err(), "{case}");
        } else {
            let messages: Vec<_> = actual.err().map(|e| e.to_string()).into_iter().collect();
            assert_eq!(json!(messages), case["violations"], "{case}");
        }
    }
}

#[test]
fn cli_contract() {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_tl3"))
        .args(["check-reply", "classifier"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let doc = json!({"inputs":inputs(),"reply":{"kind":"restart"}});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(doc.to_string().as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{:?}", result.stderr);
    assert!(result.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        json!({"violations":["restart is not offered for this request"]})
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn parsed_question_identity_and_offers(id in "\\PC{1,50}", same in any::<bool>(), offered in any::<bool>(), unclear in any::<bool>()) {
        let other = if same {id.clone()} else {format!("other:{id}")};
        let mut input = inputs();
        input["state"]["open_question"] = json!({"id": id, "kind":"effect_failed", "answers":if offered {vec!["continue"]} else {vec!["stop"]}});
        let reply = json!({"kind":"answer","question_id":other,"answer":if unclear {"unclear"} else {"continue"}});
        let bytes = serde_json::to_vec(&json!({"inputs":input,"reply":reply})).unwrap();
        let roundtrip: Value = serde_json::from_slice(&bytes).unwrap();
        let actual = validate(&roundtrip["inputs"], &roundtrip["reply"]);
        prop_assert_eq!(actual.is_ok(), id == other && (unclear || offered));
    }

    #[test]
    fn parsed_decision_membership(ids in prop::collection::vec(0u8..20, 0..20), selected in prop::collection::vec(0u8..25, 0..20), retired in any::<bool>()) {
        let mut input = inputs();
        let decisions: Vec<_> = ids.iter().map(|id| json!({"id":format!("q{id}"),"source_comment":"https://example.invalid/comment", "question":{}, "answer":"yes", "allowed_scope":[],"restrictions":[],"origin_request_version":1,"superseded_by":if retired {json!("later")} else {Value::Null}})).collect();
        input["decision_context"] = json!({"schema_version":1,"request_version":1,"decisions":decisions});
        let unique: std::collections::BTreeSet<_> = selected.iter().collect();
        let supersedes: Vec<_> = unique.iter().map(|id| format!("q{id}")).collect();
        let reply = json!({"kind":"change_request","new_text":"new text","supersedes":supersedes});
        prop_assert_eq!(validate(&input, &reply).is_ok(), unique.iter().all(|id| !retired && ids.contains(id)));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn parsed_reconciliation_exact_coverage(ids in prop::collection::vec(0u8..10, 0..20), assessments in prop::collection::vec(0u8..12, 0..20), exact in any::<bool>()) {
        let mut input = inputs();
        let assessments = if exact {ids.iter().copied().collect::<std::collections::BTreeSet<_>>().into_iter().collect()} else {assessments};
        input["reconcile_issue"] = json!({"number":1,"title":"Restart","body":null,"updated_at":"2026-11-04"});
        input["decision_context"] = json!({"schema_version":1,"request_version":1,"decisions":ids.iter().map(|id| json!({"id":format!("q{id}"),"source_comment":null,"question":{},"answer":"yes","allowed_scope":[],"restrictions":[],"origin_request_version":1,"superseded_by":"later"})).collect::<Vec<_>>()});
        let reply = json!({"kind":"reconcile_decisions","assessments":assessments.iter().map(|id|json!({"id":format!("q{id}"),"contradicted":false,"reason":"still applies"})).collect::<Vec<_>>()});
        let active: std::collections::BTreeSet<_> = ids.iter().collect();
        let assessed: std::collections::BTreeSet<_> = assessments.iter().collect();
        let expected = active == assessed && assessed.len() == assessments.len();
        prop_assert_eq!(validate(&input,&reply).is_ok(),expected);
    }
}
