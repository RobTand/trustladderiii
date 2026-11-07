use serde_json::json;
use tl3_action::{PolicyOperation, PolicyRequest, PolicyResult, policy};
use tl3_contracts::rules::{self, *};
use tl3_redaction::structured::Redactor;
fn row() -> Violation {
    Violation {
        rule_id: "proof.subject".into(),
        role: Role::Maker,
        names: vec![Name {
            kind: NameKind::Path,
            value: "test_subject.py".into(),
        }],
        bad_value: json!("bad"),
        allowed: Allowed::Finite { values: vec![] },
        repair: Repair::SourceEdit {
            path: "test_subject.py".into(),
            instruction: "Keep the complete source repair.".into(),
        },
        gating: true,
        evidence: vec![],
        evidence_truncated: 0,
    }
}
#[test]
fn producer_secrets_are_removed_before_refusal_publication() {
    let secret = "producer-secret-a7B9c3D8e2F6";
    let row = row().with_evidence(vec![Evidence::TestFailure {
        target: "//tests:subject".into(),
        test: "case[é]".into(),
        excerpt: format!("API_TOKEN={secret}"),
        log_ref: "log://failure".into(),
        receipt: "receipt".into(),
    }]);
    let mut state = Redactor::default();
    state.secrets.insert(secret.into());
    let response = policy(PolicyRequest {
        state,
        operation: PolicyOperation::Render {
            rows: vec![row],
            role: Role::Maker,
            context: RenderContext::Tool,
        },
    })
    .unwrap();
    let PolicyResult::Text(text) = response.result else {
        panic!("expected text")
    };
    assert!(!text.contains(secret));
    assert!(text.contains("[REDACTED]"));
    assert!(text.contains("Keep the complete source repair."));
    assert!(text.contains("Further checks were not run."));
}
#[test]
fn final_size_and_context_fail_explicitly() {
    let mut row = row();
    row.allowed = Allowed::Finite {
        values: vec![json!("é".repeat(rules::MESSAGE_BYTES))],
    };
    assert!(
        policy(PolicyRequest {
            state: Redactor::default(),
            operation: PolicyOperation::Render {
                rows: vec![row],
                role: Role::Maker,
                context: RenderContext::Tool
            }
        })
        .is_err()
    );
}
#[test]
fn expected_red_declaration_is_not_observed_match_evidence() {
    let declaration = json!({"subjects":["a.py::value"], "observer_support":[], "red_message":"the expected message"});
    let mut observed = declaration.clone();
    observed["observed_message"] = json!("model authored capture");
    assert!(serde_json::from_value::<tl3_contracts::ProofDeclaration>(declaration).is_ok());
    assert!(serde_json::from_value::<tl3_contracts::ProofDeclaration>(observed).is_err());
}

fn command(operation: serde_json::Value) -> serde_json::Value {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_tl3-action"))
        .env_clear()
        .arg("policy")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let request =
        json!({"state":{"secrets":[],"calls":{},"truncated":false},"operation":operation});
    writeln!(child.stdin.take().unwrap(), "{request}").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn schema_repairs_through_the_action_preserve_escaped_subjects_and_permission_union() {
    let failure = |keyword: &str,
                   schema: serde_json::Value,
                   instance: serde_json::Value,
                   path: serde_json::Value,
                   validator_value: serde_json::Value| {
        json!({
        "keyword":keyword,"validator_value":validator_value,"schema":schema,"instance":instance,
        "path":path,"schema_path":[],"context":[]})
    };
    let source = json!({"type":"object","properties":{"nested":{"type":"object","properties":{
        "a/b~c":{"enum":["fixed"]},"count":{"type":"integer","minimum":1}},"required":["a/b~c","count"]}}});
    let result = command(json!({"op":"schema_rows","errors":[
        failure("required",source["properties"]["nested"].clone(),json!({}),json!(["nested"]),json!(["a/b~c","count"]))
    ],"root_schema":source,"role":"maker"}));
    let rows = result["ok"]["result"]["value"].clone();
    assert_eq!(rows.as_array().unwrap().len(), 2);
    let text = command(json!({"op":"render","rows":rows,"role":"maker","context":"reply"}));
    let text = text["ok"]["result"]["value"].as_str().unwrap();
    assert!(text.contains("/nested/a~1b~0c"));
    assert!(text.contains("/nested/count"));
    assert!(text.contains("add"));
    let before = json!({"nested":{},"untouched":true});
    for (after, expected) in [
        (
            json!({"nested":{"a/b~c":"fixed","count":1},"untouched":true}),
            true,
        ),
        (
            json!({"nested":{"a/b~c":"fixed","count":0},"untouched":true}),
            false,
        ),
        (
            json!({"nested":{"a/b~c":"fixed","count":1},"untouched":1}),
            false,
        ),
        (
            json!({"nested":{"a/b~c":"fixed","count":1},"untouched":false}),
            false,
        ),
    ] {
        let result = command(
            json!({"op":"permits","rows":rows,"before":before,"after":after,"role":"maker"}),
        );
        assert_eq!(result["ok"]["result"]["value"], json!(expected));
    }
}

#[test]
fn nested_unsupported_schema_semantics_and_unresolved_references_are_explicit_errors() {
    for constraint in [
        json!({"properties":{"ordinary":{"items":{"x-ordered":{}}}}}),
        json!({"$defs":{"unused":{"x-memory-flags":{}}}}),
        json!({"anyOf":[{"$ref":"missing-ref.json"},{"type":"null"}]}),
        json!({"$ref":"#/$defs/missing"}),
    ] {
        let result = command(
            json!({"op":"membership","allowed":{"kind":"schema","keyword":"schema","value":constraint},"value":null}),
        );
        assert!(result.get("error").is_some(), "{result}");
        assert!(result.get("ok").is_none());
    }
    for constraint in [
        json!({"properties":{"x-ordered":{"const":{"x-memory-flags":7}}}}),
        json!({"const":{"x-ordered":{}}}),
        json!({"enum":[{"x-memory-flags":{}}]}),
    ] {
        let result = command(
            json!({"op":"membership","allowed":{"kind":"schema","keyword":"schema","value":constraint},"value":null}),
        );
        assert!(
            result.get("ok").is_some(),
            "instance keys were treated as schema keywords: {result}"
        );
    }
}

#[test]
fn complete_repair_prompts_are_redacted_and_never_clip_the_repair_tail() {
    let result = command(
        json!({"op":"repair_prompt","message":"password=producer-secret-Q7r9T3x6\nComplete repair at /checks/0/tests."}),
    );
    let text = result["ok"]["result"]["value"].as_str().unwrap();
    assert!(!text.contains("producer-secret-Q7r9T3x6"));
    assert!(text.contains("Complete repair at /checks/0/tests."));
    assert!(text.ends_with("nothing else."));
    assert!(
        command(json!({"op":"repair_prompt","message":"é".repeat(rules::MESSAGE_BYTES)}))
            .get("error")
            .is_some()
    );
}

#[test]
fn every_row_boundary_rejects_unsupported_constraints_before_publication() {
    for constraint in [
        json!({"properties":{"x":{"items":{"x-ordered":[]}}}}),
        json!({"$ref":"unavailable.json"}),
        json!({"$defs":{"unused":{"$ref":"unavailable.json"}}}),
    ] {
        let mut value = serde_json::to_value(row()).unwrap();
        value["allowed"] = json!({"kind":"schema","keyword":"schema","value":constraint});
        for operation in [
            json!({"op":"validate","rows":[value.clone()],"role":"maker"}),
            json!({"op":"render","rows":[value.clone()],"role":"maker","context":"tool"}),
            json!({"op":"permits","rows":[value],"role":"maker","before":{},"after":{}}),
        ] {
            assert!(
                command(operation.clone()).get("error").is_some(),
                "boundary accepted unsupported semantics: {operation}"
            );
        }
    }
}

#[test]
fn trace_retention_and_allocation_are_action_owned() {
    let entries: Vec<_> = (0..4100).map(|index| json!({"root":index==0,"kind":"TOOL","proto_bytes":16,"sizes":[1024],"shrunk":false})).collect();
    let result = command(
        json!({"op":"trace","request":{"op":"retain","entries":entries,"shrinking":false}}),
    );
    let decision = &result["ok"]["result"]["value"];
    let keep = decision["keep"].as_array().unwrap();
    assert_eq!(keep.len(), 4096);
    assert_eq!(keep[0], 0);
    assert_eq!(keep[1023], 1023);
    assert_eq!(keep[1024], 1028);
    assert_eq!(keep[4095], 4099);
    let result = command(json!({"op":"trace","request":{"op":"allocate","entries":[
        {"root":true,"kind":"AGENT","proto_bytes":100,"sizes":[65536],"shrunk":false},
        {"root":false,"kind":"CHAIN","proto_bytes":100,"sizes":[65536],"shrunk":false},
        {"root":false,"kind":"TOOL","proto_bytes":100,"sizes":[65536],"shrunk":false}
    ]}}));
    let limits = result["ok"]["result"]["value"]["limits"]
        .as_array()
        .unwrap();
    assert_eq!(limits[0][0], 65536);
    assert!(limits[2][0].as_u64().unwrap() >= limits[1][0].as_u64().unwrap());
    assert!(
        limits
            .iter()
            .flat_map(|items| items.as_array().unwrap())
            .map(|v| v.as_u64().unwrap())
            .sum::<u64>()
            < 1024 * 1024
    );
}
