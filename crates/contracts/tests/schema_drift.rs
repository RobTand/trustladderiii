//! The extracted contract crate still renders the committed schema (#741, #819).

use tl3_contracts as contracts;

const FIX: &str = "bazel run --config=remote //crates/contracts:tl3-contracts-schemas";

// The committed generated files, compiled in so the check stays hermetic.
const COMMITTED: &[(&str, &str)] = &[
    (
        "workers/src/tl3_workers/schemas/check-report.json",
        include_str!("../../../workers/src/tl3_workers/schemas/check-report.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/execution-failure.json",
        include_str!("../../../workers/src/tl3_workers/schemas/execution-failure.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/decision-context.json",
        include_str!("../../../workers/src/tl3_workers/schemas/decision-context.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/request.json",
        include_str!("../../../workers/src/tl3_workers/schemas/request.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/postmortem-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/postmortem-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/ingest-reply.json",
        include_str!("../../../workers/src/tl3_workers/schemas/ingest-reply.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/classifier-reply.json",
        include_str!("../../../workers/src/tl3_workers/schemas/classifier-reply.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/maker-reply.json",
        include_str!("../../../workers/src/tl3_workers/schemas/maker-reply.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/writer-reply.json",
        include_str!("../../../workers/src/tl3_workers/schemas/writer-reply.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/planner-reply.json",
        include_str!("../../../workers/src/tl3_workers/schemas/planner-reply.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/reviewer-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/reviewer-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/ingest-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/ingest-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/audit-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/audit-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/planner-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/planner-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/maker-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/maker-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/writer-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/writer-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/classifier-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/classifier-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/plan-check-provenance.json",
        include_str!("../../../workers/src/tl3_workers/schemas/plan-check-provenance.json"),
    ),
    (
        "crates/contracts/schema/violation.json",
        include_str!("../schema/violation.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/action-request.json",
        include_str!("../../../workers/src/tl3_workers/schemas/action-request.json"),
    ),
];

#[test]
fn generated_schemas_match_committed() {
    let generated = contracts::generated();
    assert_eq!(
        generated
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        COMMITTED.iter().map(|(path, _)| *path).collect::<Vec<_>>(),
        "every generated file is committed and compiled into this test"
    );
    for (file, (path, committed)) in generated.iter().zip(COMMITTED) {
        if file.text == *committed {
            continue;
        }
        let line = file
            .text
            .lines()
            .zip(committed.lines())
            .position(|(rendered, written)| rendered != written)
            .unwrap_or_else(|| file.text.lines().count().min(committed.lines().count()));
        panic!(
            "{path} no longer matches the Rust contract types (first difference at line {}: \
             rendered {:?}, committed {:?}). Never edit it by hand; regenerate with: {FIX}",
            line + 1,
            file.text.lines().nth(line),
            committed.lines().nth(line),
        );
    }
}

// #819: keep one named red/green case per remaining input root.
macro_rules! input_drift {
    ($test:ident, $role:literal) => {
        #[test]
        fn $test() {
            let path = concat!("workers/src/tl3_workers/schemas/", $role, "-input.json");
            let file = contracts::generated()
                .into_iter()
                .find(|f| f.path == path)
                .unwrap_or_else(|| panic!("missing generated input root: {path}"));
            let committed = COMMITTED.iter().find(|(p, _)| *p == path).unwrap().1;
            assert_eq!(file.text, committed, "{path}");
        }
    };
}
input_drift!(ingest_input_is_generated_byte_identically, "ingest");
input_drift!(audit_input_is_generated_byte_identically, "audit");
input_drift!(planner_input_is_generated_byte_identically, "planner");
input_drift!(maker_input_is_generated_byte_identically, "maker");
input_drift!(writer_input_is_generated_byte_identically, "writer");
input_drift!(classifier_input_is_generated_byte_identically, "classifier");

/// #926, tessera #1455: accepted RED classes survive the receipt boundary.
#[test]
fn red_classes_round_trip_and_reject_unknown_values() {
    use contracts::ProofReceipt;
    use serde_json::{Value, json};
    let mut receipt = json!({
        "rule": "subject-observer-v1", "declaration": null, "mode": "head-observer",
        "subjects": {}, "observer_sha256": {}, "support_sha256": {},
        "source_base_tree_sha256": "a".repeat(64), "base_tree_sha256": "b".repeat(64),
        "head_tree_sha256": "c".repeat(64)
    });
    let parsed: ProofReceipt = serde_json::from_value(receipt.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), receipt);
    for class in ["assertion", "added-name", "symptom"] {
        receipt["red_classes"] = json!({"test.py::test_x": class});
        let parsed: ProofReceipt = serde_json::from_value(receipt.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), receipt);
    }
    receipt["red_classes"] = json!({"test.py::test_x": "error"});
    assert!(serde_json::from_value::<ProofReceipt>(receipt.clone()).is_err());
    receipt["red_classes"] = Value::Null;
    assert!(serde_json::from_value::<ProofReceipt>(receipt).is_err());
}

/// Rust action slice 1: every refusal consumer uses one generated resolved row.
#[test]
fn refusal_row_schema_is_published_by_the_contract_owner() {
    let files = contracts::generated();
    let file = files
        .iter()
        .find(|file| file.path == "crates/contracts/schema/violation.json")
        .expect("contracts must publish the canonical resolved refusal row");
    let schema: serde_json::Value = serde_json::from_str(&file.text).unwrap();
    let required = schema["required"].as_array().unwrap();
    for field in ["role", "rule_id", "names", "allowed", "repair", "gating"] {
        assert!(required.contains(&serde_json::json!(field)), "{field}");
    }
    assert_eq!(schema["properties"]["evidence"]["maxItems"], 16);
    let encoded = file.text.as_str();
    assert!(encoded.contains("1024 UTF-8 bytes"));
    assert!(encoded.contains("4096 UTF-8 bytes"));
    for forbidden in ["Callback", "Registration", "Witness", "RuleFacts"] {
        assert!(
            !encoded.contains(forbidden),
            "executable policy leaked: {forbidden}"
        );
    }
}

/// Rust action slice 1: Python transports action requests, never defines their schema.
#[test]
fn action_request_schema_reuses_checks_and_excludes_embedded_python() {
    let files = contracts::generated();
    let file = files
        .iter()
        .find(|file| file.path == "workers/src/tl3_workers/schemas/action-request.json")
        .expect("contracts must publish the Rust action request");
    let schema: serde_json::Value = serde_json::from_str(&file.text).unwrap();
    let alternatives = schema["oneOf"].as_array().unwrap();
    let actions: Vec<_> = alternatives
        .iter()
        .map(|variant| variant["properties"]["action"]["const"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["checks", "tests", "generator"]);
    assert!(schema["$defs"]["CheckRequest"].is_object());
    assert!(schema["$defs"].get("Check").is_some());
    assert!(!file.text.contains("embedded_python"));
}

#[test]
fn canonical_check_requires_original_goal_indices_and_keeps_expected_proof() {
    use serde_json::json;
    use contracts::{Check, GoalId};
    let value = json!({"name":"observes the change", "tests":["tests/test_a.py::test_a"],
        "kind":"demonstrating", "goal_ids":[2, 0], "proof":{
            "subjects":["a.py::value"],"observer_support":[],"red_message":"expected assertion"}});
    let check: Check = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(check.goal_ids, vec![GoalId(2), GoalId(0)]);
    assert_eq!(serde_json::to_value(&check).unwrap(), value);
    for field in ["observed_message", "matches", "receipt", "captured"] {
        let mut untrusted = value.clone();
        untrusted["proof"][field] = json!("fabricated");
        assert!(serde_json::from_value::<Check>(untrusted).is_err(), "{field}");
    }
    let mut absent = value.clone();
    absent.as_object_mut().unwrap().remove("goal_ids");
    assert!(serde_json::from_value::<Check>(absent).is_err());
    for ids in [json!([-1]), json!([0,0]), json!(["goal-1"]), json!(null)] {
        let mut invalid = value.clone(); invalid["goal_ids"] = ids;
        assert!(serde_json::from_value::<Check>(invalid).is_err());
    }
    let mut alias = value.clone();
    alias.as_object_mut().unwrap().remove("tests");
    alias["node_ids"] = json!(["tests/test_a.py::test_a"]);
    assert!(serde_json::from_value::<Check>(alias).is_err());
    assert!(contracts::checks::admit_plan(&[check.clone()], 3, &[GoalId(0), GoalId(2)]).is_ok());
    assert!(contracts::checks::admit_plan(&[check.clone()], 3, &[GoalId(1)]).is_err());
    assert!(contracts::checks::admit_plan(&[check.clone(),check.clone()],3,&[]).is_err());
    assert!(contracts::checks::admit_plan(&[check.clone()],2,&[]).is_err());
    let mut other = check.clone(); other.name = "another observation".into();
    assert!(contracts::checks::admit_plan(&[check.clone(),other],3,&[GoalId(0)]).is_ok());
    let mut control = check; control.goal_ids.clear();
    assert!(contracts::checks::admit_plan(&[control],3,&[]).is_err());
}

#[test]
fn model_schema_reuses_check_and_excludes_observed_evidence() {
    use serde_json::json;
    let schema = contracts::roles::planner_reply::schema_value();
    let check = &schema["$defs"]["Check"];
    assert!(check["required"].as_array().unwrap().contains(&json!("goal_ids")));
    assert!(check["required"].as_array().unwrap().contains(&json!("name")));
    assert_eq!(check["properties"]["kind"]["enum"], json!(["demonstrating","regression","behavior-preserving"]));
    assert!(schema["$defs"]["ProofDeclaration"]["required"].as_array().unwrap().contains(&json!("red_message")));
    for absent in ["ProofReceipt", "RedMessageMatch", "CheckDeclaration", "ProofSelection"] {
        assert!(schema["$defs"].get(absent).is_none(), "{absent}");
    }
}
