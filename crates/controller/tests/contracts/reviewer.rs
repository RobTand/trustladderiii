use super::{check_result, proof_declaration, shared_generation_receipt, shared_reply_validator};
use proptest::prelude::*;
use serde_json::{Value, json};
use tl3_contracts::{bounded::*, roles::reviewer_input::*};

fn minimal() -> Value {
    json!({"goals":[{"id":"goal-1","description":"Do work","verification":"test"}],
        "base_sha":"a".repeat(40),"head_sha":"b".repeat(40),"diff":"", "checks":[],"standing_rules":[]})
}

fn assert_wire(value: Value) {
    let validator = shared_reply_validator("reviewer", None);
    assert!(
        validator.is_valid(&value),
        "{:?}",
        validator
            .iter_errors(&value)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
    let typed: ReviewerInput = serde_json::from_value(value.clone()).unwrap();
    let encoded = serde_json::to_value(&typed).unwrap();
    assert_eq!(encoded, value);
    assert_eq!(
        serde_json::to_string(&encoded).unwrap(),
        serde_json::to_string(&value).unwrap()
    );
    assert_ne!(
        serde_json::to_string(&typed).unwrap(),
        serde_json::to_string(&value).unwrap()
    );
    assert!(validator.is_valid(&encoded));
    assert_eq!(
        serde_json::from_value::<ReviewerInput>(encoded).unwrap(),
        typed
    );
}

#[test]
fn reviewer_generation_and_serialization_order_are_pinned() {
    assert_eq!(
        generated().unwrap().text,
        include_str!("../../../../workers/src/tl3_workers/schemas/reviewer-input.json")
    );
    assert_wire(minimal());
}

#[test]
fn reviewer_presence_unknown_fields_and_conditionals_match_schema() {
    let mut cases = Vec::new();
    for key in [
        "generation_evidence",
        "memories",
        "memory_note",
        "context_size",
        "check_base_sha",
        "scope_diff",
        "unresolved_displaced_failures",
        "edited_selected_tests",
        "issue",
        "test_evidence",
        "review_fixes",
    ] {
        let mut value = minimal();
        value[key] = Value::Null;
        cases.push(value);
    }
    for key in [
        "goals",
        "base_sha",
        "head_sha",
        "diff",
        "checks",
        "standing_rules",
    ] {
        let mut value = minimal();
        value.as_object_mut().unwrap().remove(key);
        cases.push(value);
    }
    let mut value = minimal();
    value["extra"] = json!(true);
    cases.push(value);
    for stage in [None, Some("maker"), Some("checking"), Some("publishing")] {
        for has_publication in [false, true] {
            let mut value = minimal();
            if let Some(stage) = stage {
                value["goals"][0]["stage"] = json!(stage);
            }
            if has_publication {
                value["goals"][0]["publication"] =
                    json!({"non_draft":true,"body_lines":[],"proof":false});
            }
            cases.push(value);
        }
    }
    for tests in [false, true] {
        for reason in [false, true] {
            let mut value = minimal();
            let mut finding = json!({"id":"f","report":"report"});
            if tests {
                finding["tests"] = json!(["test_a"]);
            }
            if reason {
                finding["reason"] = json!("why");
            }
            value["review_fixes"] = json!({"reviewed_sha":"a".repeat(40),"findings":[finding]});
            cases.push(value);
        }
    }
    for (pointer, replacement) in [
        ("/goals/0/id", json!("")),
        ("/goals/0/verification", json!("other")),
        ("/base_sha", json!("abc")),
        ("/goals", json!([])),
    ] {
        let mut value = minimal();
        *value.pointer_mut(pointer).unwrap() = replacement;
        cases.push(value);
    }
    for value in cases {
        assert_eq!(
            serde_json::from_value::<ReviewerInput>(value.clone()).is_ok(),
            shared_reply_validator("reviewer", None).is_valid(&value),
            "{value}"
        );
    }
}

// #819 P-schema-sound: construct the public root and all input-owned conditional
// variants, then validate against the published schema and parse the serialized value.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn typed_reviewers_satisfy_schema_and_round_trip(text in ".{1,32}", bits in any::<u16>(), n in any::<u64>(), checks in prop::collection::vec(check_result(),0..3), proof in proof_declaration()) {
        let mut value = minimal();
        value["diff"]=json!(text); value["checks"]=json!(checks);
        if bits & 1 != 0 {value["goals"][0]["stage"]=json!("publishing"); value["goals"][0]["publication"]=json!({"branch":"topic","branch_prefix":"topic/","non_draft":true,"body_lines":["Publish the change"],"proof":true});}
        if bits & 2 != 0 {value["goals"][0]["proof_obligation"]=json!({"kind":"assertion","observer":"new","subjects":["test_a.py::test_a"],"source":"issue","quote":text});}
        if bits & 4 != 0 {value["memories"]=json!([{"id":"m","kind":"lesson","repo":"o/r","content":text}]); value["memory_note"]=json!(text);}
        if bits & 8 != 0 {value["context_size"]=json!(n.max(1));value["check_base_sha"]=json!("c".repeat(64));value["scope_diff"]=json!(text);value["issue"]=json!({"title":text,"body":text});}
        if bits & 16 != 0 {value["unresolved_displaced_failures"]=json!([text]);value["edited_selected_tests"]=json!([text]);}
        if bits & 32 != 0 {value["review_fixes"]=json!({"reviewed_sha":"d".repeat(40),"findings":[{"id":"f","report":text,"tests":[text],"proof":proof}]});}
        if bits & 64 != 0 {value["review_fixes"]=json!({"reviewed_sha":"d".repeat(40),"findings":[{"id":"f","report":text,"reason":"No tests apply","proof":proof}]});}
        if bits & 128 != 0 {value["generation_evidence"]=shared_generation_receipt(0.5,n%129,bits&256 != 0);}
        if bits & 512 != 0 {value["test_evidence"]=json!([{"tests":[text],"base":false,"repeat":1.0,"parallel":0,"status":"completed","tree":"a".repeat(40),"fresh":true,"totals":{"passed":1,"failed":0,"error":0,"skipped":0},"receipts":[{"action_key":"a".repeat(64),"status":"executed","exit_code":0,"receipt_sha256":"b".repeat(64),"log_sha256":"c".repeat(64)}]}]);}
        let parsed: ReviewerInput=serde_json::from_value(value.clone()).unwrap();
        let typed=ReviewerInput {goals:List::new(parsed.goals.to_vec()).unwrap(), standing_rules:vec![StandingRule{id:Text::new("rule").unwrap(),description:Text::new(text).unwrap()}], ..parsed};
        let mut expected=value; expected["standing_rules"]=serde_json::to_value(&typed.standing_rules).unwrap();
        assert_wire(expected);
        prop_assert!(shared_reply_validator("reviewer",None).is_valid(&serde_json::to_value(&typed).unwrap()));
        prop_assert_eq!(serde_json::from_value::<ReviewerInput>(serde_json::to_value(&typed).unwrap()).unwrap(),typed);
    }
}

// #819: every schema-valid report value retains message presence through typed parsing.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn shared_reports_preserve_absent_empty_and_nonempty_messages(mut test in super::test_result(), message in ".{1,64}") {
        use tl3_contracts::{CheckReport, CheckResult, TestResult};
        static VALIDATORS: std::sync::OnceLock<[jsonschema::Validator;3]>=std::sync::OnceLock::new();
        let validators=VALIDATORS.get_or_init(||[super::check_report_validator(Some("TestResult")),super::check_report_validator(Some("CheckResult")),super::check_report_validator(None)]);
        for presence in 0..3 {
            if presence==0 {test.as_object_mut().unwrap().remove("message");}
            else {test["message"]=json!(if presence==1 {""} else {&message});}
            let result=json!({"name":"check","kind":"regression","verdict":"pass","custody":"in-process","tests":[test]});
            let report=json!({"schema":"tl3.checks.v1","effect_key":"check:0","base":"a".repeat(40),"head":"b".repeat(40),"reviewed":null,"results":[result]});
            prop_assert!(validators[0].is_valid(&test));prop_assert!(validators[1].is_valid(&result));prop_assert!(validators[2].is_valid(&report));
            prop_assert_eq!(serde_json::to_value(serde_json::from_value::<TestResult>(test.clone()).unwrap()).unwrap(),test.clone());
            prop_assert_eq!(serde_json::to_value(serde_json::from_value::<CheckResult>(result.clone()).unwrap()).unwrap(),result);
            prop_assert_eq!(serde_json::to_value(serde_json::from_value::<CheckReport>(report.clone()).unwrap()).unwrap(),report);
        }
    }
}
