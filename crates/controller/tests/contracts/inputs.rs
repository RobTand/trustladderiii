//! #819: all remaining input roots preserve schema values and sorted producer bytes.
use proptest::prelude::*;
use serde_json::{Value, json};
use tl3_contracts::roles::*;

fn sound<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
    role: &str,
    value: Value,
) {
    let validator = super::shared_reply_validator(&format!("{role}-input"), None);
    assert!(
        validator.is_valid(&value),
        "{role}: {:?}: {value}",
        validator
            .iter_errors(&value)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
    let typed: T =
        serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{role}: {e}: {value}"));
    let encoded = serde_json::to_value(&typed).unwrap();
    assert_eq!(
        serde_json::to_vec(&encoded).unwrap(),
        serde_json::to_vec(&value).unwrap()
    );
    assert_ne!(
        serde_json::to_vec(&typed).unwrap(),
        serde_json::to_vec(&value).unwrap()
    );
    assert!(validator.is_valid(&encoded));
    assert_eq!(serde_json::from_value::<T>(encoded).unwrap(), typed);
}
fn issue(text: &str) -> Value {
    json!({"number":1.0,"title":text,"body":"","author":"owner","comments":[{"author":"owner","body":text}]})
}
fn repo() -> Value {
    json!({"readme":"","file_list":["src.py"],"file_count":1.0,"tree_summary":"src.py"})
}
fn forecast() -> Value {
    json!({"fit_tokens":100,"context_tokens":1000,"overhead_tokens":{"system":10.0}})
}
fn brief(text: &str) -> Value {
    json!({"kind":"brief","brief":text,"goals":[{"description":text,"verification":"test"}],"memory_ids":[]})
}
fn memories(text: &str) -> Value {
    json!([{"id":"memory-1","kind":"lesson","repo":"o/r","content":text}])
}
fn ingest(text: &str, bits: u16) -> Value {
    let mut v = json!({"issue":issue(text),"repo":repo()});
    if bits & 1 != 0 {
        v["memories"] = memories(text);
        v["memory_note"] = json!(text);
    }
    if bits & 2 != 0 {
        v["forecast"] = forecast();
        v["forecast"]["generated_files"] = json!(["out/**"]);
        v["forecast"]["generator"] = json!("generate");
        v["forecast"]["generator_available"] = json!(true);
        v["forecast"]["generator_timeout_s"] = json!(600.0);
    }
    if bits & 4 != 0 {
        v["publication_branch"] =
            json!({"repository":"o/r","issue":1,"node":0,"template":"tl3/{node}","branch":"tl3/0"});
        v["protected_paths"] = json!(["protected/**"]);
        v["audit_objections"] = json!([text]);
    }
    v
}
fn audit(text: &str, bits: u16) -> Value {
    let mut v = json!({"issue":issue(text),"repo":repo(),"ingest":brief(text)});
    if bits & 1 != 0 {
        v["memories"] = memories(text);
    }
    if bits & 2 != 0 {
        v["protected_paths"] = json!(["protected/**"]);
    }
    v
}
fn planner(text: &str, bits: u16) -> Value {
    let mut v = json!({"must_split":bits&1!=0,"findings":[text],"protected_paths":[],"ingest":brief(text),"forecast":forecast()});
    if bits & 2 != 0 {
        v["memories"] = memories(text);
        v["memory_note"] = json!(text);
        v["base_sha"] = json!("a".repeat(40));
        v["tests"] = tests();
    }
    if bits & 4 != 0 {
        v["repair_plan_consistency"] = json!(false);
        v["always_tests"] = json!(["rules.py"]);
        v["audit_evidence_contract"] = json!([]);
        v["tool_capabilities"] = json!([]);
    }
    v
}
fn tests() -> Value {
    json!({"image":format!("registry/image@sha256:{}","a".repeat(64)),"execution":{"backend":"flux","profile":"/fixture/execution.json"},"cpus":1.0,"mem_gb":1,"quiet_seconds":1})
}
fn maker(text: &str, bits: u16) -> Value {
    let mut v = json!({"brief":text,"goals":[{"id":"g","description":text}],"footprint":["src.py"],"plan":{"files":["src.py"],"checks":[{"name":"check","kind":"regression","tests":["tests.py::test_a"],"goal_ids":[0]}]},"attempt":1.0,"round":0.0,"tests":tests(),"context_size":1});
    if bits & 1 != 0 {
        v["round"] = json!(1);
        v["feedback"] = json!([{"kind":"failed_check","id":"f","reviewed_sha":"a".repeat(40),"resolved_round":1,"report":text,"goal_ids":[],"failed_tests":[{"test":"test_a","message":text}],"outside_paths":["src.py"],"outside_paths_total":1,"ci_checks":[{"name":"CI","summary":text,"url":null}]}]);
    }
    if bits & 2 != 0 {
        v["goals"][0]["verification"] = json!("test");
        v["goals"][0]["stage"] = json!("publishing");
        v["goals"][0]["publication"] = json!({"branch":"topic","branch_prefix":"topic/","non_draft":true,"body_lines":["Publish"],"proof":true});
    }
    if bits & 4 != 0 {
        v["hindsight"] = json!({"endpoint":"https://","bank":"o/r"});
        v["forecast"] = forecast();
        v["current_review_findings"] =
            json!([{"id":"f","reviewed_sha":"a".repeat(40),"opaque":{"nested":[1,null]}}]);
    }
    v
}
fn classifier(text: &str, bits: u16) -> Value {
    let mut v = json!({"comment":{"author":"owner","body":text},"state":{"phase":"Planning","closed":false}});
    match bits % 3 {
        0 => {}
        1 => v["state"]["open_question"] = Value::Null,
        _ => {
            v["state"]["open_question"] =
                json!({"id":"q","kind":"cant_meet","answers":["continue","stop"]})
        }
    }
    if bits & 4 != 0 {
        v["comment"]["delegated_by"] = json!("owner");
        v["state"]["restartable"] = json!(true);
        v["conversation"] = json!({"comment_id":"c","comment":text,"question":text});
        v["question_context"] = json!({"opaque":[1,null]});
        v["selected_answer"] = json!({"kind":"stop"});
        v["reconcile_issue"] = json!({"number":-1.0,"title":text,"body":null,"updated_at":"now","opaque":{"nested":[null,true]}});
    }
    v
}
fn writer(text: &str, bits: u16) -> Value {
    let (kind, facts) = match bits % 4 {
        0 => (
            "question",
            json!({"issue_title":text,"request":text,"obstacle":text,"options":[{"action":"continue","outcome":"run"},{"action":"stop","outcome":"stop"}],"recommended_option":0.0,"recommendation_reason":text,"how_to_answer":text,"findings":{"source":"planner","decision":text,"pieces":[{"brief":text,"files":[]}]}}),
        ),
        1 => (
            "note",
            json!({"issue_title":text,"request":text,"change":text,"publication_diagnostic":{"opaque":[1,null]},"occurrences":1.0}),
        ),
        2 => (
            "summary",
            json!({"issue_title":text,"request":text,"outcome":text,"unmet_goals":[text],"evidence_status":"captured","merge_status":"not_recorded","pull_request":null,"evidence":{"opaque":[null]},"findings":{"source":"maker","report":text}}),
        ),
        _ => (
            "clarification",
            json!({"issue_title":text,"request":text,"question":text,"comment_excerpt":text}),
        ),
    };
    let mut v = json!({"kind":kind,"facts":facts});
    if bits & 8 != 0 {
        v["violations"] = json!([text]);
    }
    v
}
macro_rules! role_property {
    ($name:ident,$typ:ty,$role:literal,$build:ident) => {
        proptest! {
            #![proptest_config(ProptestConfig::with_cases(128))]
            #[test]
            fn $name(suffix in ".{0,32}",bits in any::<u16>()) {
                let text=format!("evidence {suffix}");sound::<$typ>($role,$build(&text,bits));
            }
        }
    };
}
role_property!(
    ingest_schema_sound,
    ingest_input::IngestInput,
    "ingest",
    ingest
);
role_property!(audit_schema_sound, audit_input::AuditInput, "audit", audit);
role_property!(
    planner_schema_sound,
    planner_input::PlannerInput,
    "planner",
    planner
);
role_property!(maker_schema_sound, maker_input::MakerInput, "maker", maker);
role_property!(
    writer_schema_sound,
    writer_input::WriterInput,
    "writer",
    writer
);
role_property!(
    classifier_schema_sound,
    classifier_input::ClassifierInput,
    "classifier",
    classifier
);

#[test]
fn input_schema_values_remain_identical() {
    fn differences(path: &str, actual: &Value, expected: &Value, found: &mut Vec<String>) {
        match (actual, expected) {
            (Value::Object(a), Value::Object(e)) => {
                for key in a
                    .keys()
                    .chain(e.keys())
                    .collect::<std::collections::BTreeSet<_>>()
                {
                    differences(
                        &format!("{path}/{key}"),
                        a.get(key).unwrap_or(&Value::Null),
                        e.get(key).unwrap_or(&Value::Null),
                        found,
                    );
                }
            }
            (Value::Array(a), Value::Array(e)) if a.len() == e.len() => {
                for (index, (a, e)) in a.iter().zip(e).enumerate() {
                    differences(&format!("{path}/{index}"), a, e, found);
                }
            }
            _ if actual != expected => {
                found.push(format!("{path}: generated={actual}, committed={expected}"))
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for (role, schema) in [
        ("ingest", ingest_input::schema()),
        ("audit", audit_input::schema()),
        ("planner", planner_input::schema()),
        ("maker", maker_input::schema()),
        ("writer", writer_input::schema()),
        ("classifier", classifier_input::schema()),
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../workers/src/tl3_workers/schemas/{role}-input.json"
        ));
        let committed: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        differences(role, &schema, &committed, &mut found);
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn maker_conditionals_and_explicit_nulls_stay_invalid() {
    let mut cases = Vec::new();
    let mut value = maker("evidence", 0);
    value["round"] = json!(1);
    cases.push(value);
    let mut value = maker("evidence", 0);
    value["goals"][0]["stage"] = json!("publishing");
    cases.push(value);
    let mut value = maker("evidence", 1);
    value["feedback"][0]
        .as_object_mut()
        .unwrap()
        .remove("reviewed_sha");
    cases.push(value);
    let mut value = maker("evidence", 0);
    value["check_base_sha"] = Value::Null;
    cases.push(value);
    for value in cases {
        assert!(!super::shared_reply_validator("maker-input", None).is_valid(&value));
        assert!(serde_json::from_value::<maker_input::MakerInput>(value).is_err());
    }
}

#[test]
fn map_annotation_transform_only_removes_permissive_true() {
    let mut schema = schemars::json_schema!({"type":"object","additionalProperties":false});
    tl3_contracts::render::omit_open_additional_properties(&mut schema);
    assert_eq!(schema.get("additionalProperties"), Some(&json!(false)));
    let mut schema = schemars::json_schema!({"type":"object","additionalProperties":true});
    tl3_contracts::render::omit_open_additional_properties(&mut schema);
    assert!(schema.get("additionalProperties").is_none());
}

/// #819: the new boundary types retain the controller schema's acceptance.
#[test]
fn boundary_values_match_controller_schema() {
    fn agrees<T: serde::de::DeserializeOwned>(role: &str, value: Value) {
        let accepted =
            super::shared_reply_validator(&format!("{role}-input"), None).is_valid(&value);
        assert_eq!(
            serde_json::from_value::<T>(value.clone()).is_ok(),
            accepted,
            "{role}: {value}"
        );
    }
    for footprint in [
        json!(["x.py", "a/b.py"]),
        json!([]),
        json!(["./x.py"]),
        json!(["a//b.py"]),
        json!(["dir/"]),
        json!(["a/./b.py"]),
        json!(["x.py", "x.py"]),
        json!(["../x.py"]),
        json!(["/x.py"]),
        json!([""]),
        json!(["bad\0path"]),
        json!([".git/config"]),
    ] {
        let mut value = maker("evidence", 0);
        value["footprint"] = footprint;
        agrees::<maker_input::MakerInput>("maker", value);
    }
    for image in [
        format!("sha256:{}", "0".repeat(64)),
        format!("registry/a_b:tag@sha256:{}", "a".repeat(64)),
        format!("répo@sha256:{}", "a".repeat(64)),
        format!("repo@sha256:{}", "A".repeat(64)),
        format!("repo@sha256:{}", "a".repeat(63)),
        format!("@sha256:{}", "a".repeat(64)),
    ] {
        let mut value = maker("evidence", 0);
        value["tests"]["image"] = json!(image);
        agrees::<maker_input::MakerInput>("maker", value);
    }
    for overhead in [
        json!({}),
        json!({"":0}),
        json!({"system":0.0}),
        json!({"system":-1}),
    ] {
        let mut value = planner("evidence", 0);
        value["forecast"]["overhead_tokens"] = overhead;
        agrees::<planner_input::PlannerInput>("planner", value);
    }
    for url in [
        Value::Null,
        json!(""),
        json!("https://example.invalid/"),
        json!("x".repeat(4097)),
    ] {
        let mut value = maker("evidence", 1);
        value["feedback"][0]["ci_checks"][0]["url"] = url;
        agrees::<maker_input::MakerInput>("maker", value);
    }
    let mut value = maker("evidence", 1);
    value["feedback"][0]["ci_checks"][0]
        .as_object_mut()
        .unwrap()
        .remove("url");
    agrees::<maker_input::MakerInput>("maker", value);
}
