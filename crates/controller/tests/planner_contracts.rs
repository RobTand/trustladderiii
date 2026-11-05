//! #820: prompt/schema identity and proof omission parity at the planner boundary.
use serde_json::{Value, json};
use tl3_controller::contracts::roles::planner_reply::{self, PlannerReply, Proof};

#[test]
fn planner_schema_is_byte_identical_and_registered_for_drift() {
    let committed = include_str!("../../../workers/src/tl3_workers/schemas/planner-reply.json");
    assert_eq!(planner_reply::schema_text(), committed);
    let generated = tl3_controller::contracts::generated();
    assert!(
        generated
            .iter()
            .any(|file| file.path.ends_with("/planner-reply.json") && file.text == committed)
    );
}

#[test]
fn planner_proof_omissions_are_explicit_and_round_trip() {
    for partial in [
        json!({}),
        json!({"subjects":[]}),
        json!({"observer_support":[]}),
        json!({"red_message":"assert status"}),
    ] {
        let proof: Proof = serde_json::from_value(partial.clone()).unwrap();
        assert!(proof.complete().is_none());
        assert_eq!(serde_json::to_value(proof).unwrap(), partial);
    }
    let complete = json!({"subjects":[],"observer_support":[],"red_message":"assert status"});
    let proof: Proof = serde_json::from_value(complete.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(proof.complete().expect("all fields declared")).unwrap(),
        complete
    );
}

#[test]
fn planner_variants_round_trip_and_reject_unknown_fields() {
    let cases = [
        json!({"kind":"leaf","files":[],"checks":[],"no_design_questions":true,"lands_on_its_own":false}),
        json!({"kind":"split","reason":"multiple_repos","pieces":[]}),
        json!({"kind":"cant_meet","report":"No useful change.","files":[]}),
    ];
    for value in cases {
        let typed: PlannerReply = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), value);
        let mut extra = value;
        extra["unknown"] = json!(true);
        assert!(serde_json::from_value::<PlannerReply>(extra).is_err());
    }
    for invalid in [
        json!({"kind":"leaf"}),
        json!({"kind":"split","reason":"invented","pieces":[]}),
        json!({"kind":"cant_meet","report":"","files":[]}),
        Value::Null,
    ] {
        assert!(serde_json::from_value::<PlannerReply>(invalid).is_err());
    }
}

#[test]
fn planner_schema_values_match_the_legacy_schema() {
    fn differences(actual: &Value, expected: &Value, path: &str, errors: &mut Vec<String>) {
        match (actual, expected) {
            (Value::Object(a), Value::Object(b)) => {
                let keys: std::collections::BTreeSet<_> = a.keys().chain(b.keys()).collect();
                for key in keys {
                    differences(
                        a.get(key).unwrap_or(&Value::Null),
                        b.get(key).unwrap_or(&Value::Null),
                        &format!("{path}/{key}"),
                        errors,
                    );
                }
            }
            (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
                for (index, (a, b)) in a.iter().zip(b).enumerate() {
                    differences(a, b, &format!("{path}/{index}"), errors);
                }
            }
            (a, b) if a != b => errors.push(format!("{path}: generated {a}, expected {b}")),
            _ => {}
        }
    }
    let expected: Value = serde_json::from_str(include_str!(
        "../../../workers/src/tl3_workers/schemas/planner-reply.json"
    ))
    .unwrap();
    let mut errors = Vec::new();
    differences(&planner_reply::schema(), &expected, "", &mut errors);
    assert!(
        errors.is_empty(),
        "{}",
        errors
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

use proptest::prelude::*;
proptest! {
    #![proptest_config(ProptestConfig {cases:64, ..ProptestConfig::default()})]
    #[test]
    fn planner_typed_replies_round_trip(
        path in "[a-z]{1,16}\\.py", count in 0u32..100_000,
        no_questions in any::<bool>(), own in any::<bool>(), declared in any::<bool>()
    ) {
        let mut value=json!({"kind":"leaf","files":[path],"checks":[],
            "no_design_questions":no_questions,"lands_on_its_own":own});
        if declared {value["estimated_exploration_turns"]=json!(count);}
        let typed:PlannerReply=serde_json::from_value(value.clone()).unwrap();
        prop_assert_eq!(serde_json::to_value(typed).unwrap(),value);
    }
}

#[test]
fn planner_proof_fields_reject_null_and_preserve_declared_empty_lists() {
    for field in ["subjects", "observer_support", "red_message"] {
        let mut value = json!({});
        value[field] = Value::Null;
        assert!(serde_json::from_value::<Proof>(value).is_err(), "{field}");
    }
    assert!(serde_json::from_value::<Proof>(json!({"extra":"field"})).is_err());
    let proof: Proof = serde_json::from_value(json!({"subjects":[]})).unwrap();
    assert!(!proof.subjects.is_undeclared());
    assert!(proof.observer_support.is_undeclared());
    assert!(proof.red_message.is_undeclared());
}

#[test]
fn planner_goal_ids_use_json_number_equality() {
    let mut reply = json!({"kind":"leaf","files":[],"checks":[
        {"kind":"regression","node_ids":[],"goal_ids":[]}],
        "no_design_questions":true,"lands_on_its_own":true});
    for ids in [
        "[1, 1.0]",
        "[0, -0.0]",
        "[18446744073709551615, 18446744073709551615]",
    ] {
        reply["checks"][0]["goal_ids"] = serde_json::from_str(ids).unwrap();
        assert!(
            serde_json::from_value::<PlannerReply>(reply.clone()).is_err(),
            "duplicate goal IDs {ids}"
        );
    }
    // Converting both integers to f64 would incorrectly collapse these distinct IDs.
    reply["checks"][0]["goal_ids"] =
        serde_json::from_str("[9007199254740993, 9007199254740992.0]").unwrap();
    let typed: PlannerReply = serde_json::from_value(reply.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), reply);
}
