use serde_json::json;
use tl3_action::structured::Redactor;
use tl3_action::{PolicyOperation, PolicyRequest, PolicyResult, policy};
use tl3_contracts::rules::{self, *};
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

fn schema_fact(
    keyword: &str,
    value: serde_json::Value,
    schema: serde_json::Value,
    instance: serde_json::Value,
    path: serde_json::Value,
) -> serde_json::Value {
    json!({"keyword":keyword,"validator_value":value,"schema":schema,"instance":instance,
        "path":path,"schema_path":[],"context":[]})
}

#[test]
fn declared_constraints_have_identical_direct_and_repair_permission_membership() {
    for (keyword, constraint, bad, accepted, rejected) in [
        ("const", json!(true), json!(false), json!(true), json!(1)),
        (
            "enum",
            json!([1, "one"]),
            json!(null),
            json!(1),
            json!(true),
        ),
        (
            "pattern",
            json!("^fixed$"),
            json!("bad"),
            json!("fixed"),
            json!("unfixed"),
        ),
        ("type", json!("integer"), json!("1"), json!(1), json!(true)),
        (
            "minLength",
            json!(3),
            json!("x"),
            json!("é中💡"),
            json!("é中"),
        ),
    ] {
        let source = json!({"type":"object","properties":{"value":{keyword:constraint}}});
        let rows = command(json!({"op":"schema_rows","errors":[schema_fact(keyword,
            constraint.clone(),source["properties"]["value"].clone(),bad.clone(),json!(["value"]))],
            "root_schema":source,"role":"maker"}))["ok"]["result"]["value"]
            .clone();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        for (candidate, expected) in [(accepted, true), (rejected, false)] {
            let membership =
                command(json!({"op":"membership","allowed":rows[0]["allowed"],"value":candidate}));
            assert_eq!(
                membership["ok"]["result"]["value"], expected,
                "{keyword}: {membership}"
            );
            let permission = command(
                json!({"op":"permits","rows":rows,"before":{"value":bad,"kept":7},
                "after":{"value":candidate,"kept":7},"role":"maker"}),
            );
            assert_eq!(
                permission["ok"]["result"]["value"], expected,
                "{keyword}: {permission}"
            );
        }
    }
}

#[test]
fn extra_field_repair_removes_only_unmatched_properties() {
    let source = json!({"type":"object","properties":{"kept":{"type":"integer"}},
        "patternProperties":{"^x-":{"type":"string"}},"additionalProperties":false});
    let before = json!({"kept":1,"x-valid":"text","a/b~c":false});
    let result = command(
        json!({"op":"schema_rows","errors":[schema_fact("additionalProperties",
        json!(false),source.clone(),before.clone(),json!([]))],"root_schema":source,"role":"maker"}),
    );
    let rows = &result["ok"]["result"]["value"];
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["repair"]["pointer"], "/a~1b~0c");
    for (after, expected) in [
        (json!({"kept":1,"x-valid":"text"}), true),
        (json!({"kept":2,"x-valid":"text"}), false),
        (json!({"kept":1}), false),
    ] {
        let result = command(
            json!({"op":"permits","rows":rows,"before":before,"after":after,"role":"maker"}),
        );
        assert_eq!(result["ok"]["result"]["value"], expected);
    }
}

#[test]
fn composite_reference_repairs_keep_local_definitions_and_every_independent_row() {
    let source = json!({"$defs":{"choice":{"type":"string","pattern":"^fixed$"}},
        "type":"object","properties":{"value":{"anyOf":[{"$ref":"#/$defs/choice"},{"type":"null"}]}}});
    let constraint = &source["properties"]["value"];
    let result = command(
        json!({"op":"schema_rows","errors":[schema_fact("anyOf",constraint["anyOf"].clone(),
        constraint.clone(),json!(17),json!(["value"]))],"root_schema":source,"role":"maker"}),
    );
    let rows = &result["ok"]["result"]["value"];
    assert_eq!(rows.as_array().unwrap().len(), 1);
    for (after, expected) in [
        (json!("fixed"), true),
        (json!(null), true),
        (json!("bad"), false),
        (json!(true), false),
    ] {
        let result = command(
            json!({"op":"permits","rows":rows,"before":{"value":17},"after":{"value":after},"role":"maker"}),
        );
        assert_eq!(result["ok"]["result"]["value"], expected, "{result}");
    }
    let source = json!({"type":"object","properties":(0..30).map(|n|(format!("field-{n}"),json!({"const":"fixed"}))).collect::<serde_json::Map<_,_>>()});
    let errors: Vec<_> = (0..30)
        .map(|n| {
            schema_fact(
                "const",
                json!("fixed"),
                json!({"const":"fixed"}),
                json!("bad"),
                json!([format!("field-{n}")]),
            )
        })
        .collect();
    let result =
        command(json!({"op":"schema_rows","errors":errors,"root_schema":source,"role":"maker"}));
    let rows = &result["ok"]["result"]["value"];
    assert_eq!(rows.as_array().unwrap().len(), 30);
    let result = command(json!({"op":"render","rows":rows,"role":"maker","context":"reply"}));
    let text = result["ok"]["result"]["value"].as_str().unwrap();
    for n in 0..30 {
        assert!(
            text.contains(&format!("/field-{n}\"")),
            "missing complete repair {n}"
        );
    }
}

#[test]
fn evidence_array_repairs_and_invalid_resolved_repairs_use_the_canonical_boundary() {
    let source = json!({"type":"object","properties":{"evidence_contract":{"type":"array"}},"required":["evidence_contract"]});
    let result = command(
        json!({"op":"schema_rows","errors":[schema_fact("required",json!(["evidence_contract"]),
        source.clone(),json!({}),json!([]))],"root_schema":source,"role":"ingest"}),
    );
    let rows = &result["ok"]["result"]["value"];
    assert_eq!(rows.as_array().unwrap().len(), 1);
    let rendered = command(json!({"op":"render","rows":rows,"role":"ingest","context":"reply"}));
    let text = rendered["ok"]["result"]["value"].as_str().unwrap();
    assert!(text.contains("/evidence_contract") && text.contains("array") && text.contains("add"));
    for (candidate, expected) in [(json!([]), true), (json!("not an array"), false)] {
        let result = command(
            json!({"op":"permits","rows":rows,"before":{},"after":{"evidence_contract":candidate},"role":"ingest"}),
        );
        assert_eq!(result["ok"]["result"]["value"], expected);
    }
    let mut row = serde_json::to_value(row()).unwrap();
    for repair in [json!(null), json!({"kind":"cant_meet","reason":""})] {
        row["repair"] = repair;
        assert!(
            command(json!({"op":"validate","rows":[row.clone()],"role":"maker"}))
                .get("error")
                .is_some()
        );
    }
}

#[test]
fn trace_labels_are_redacted_and_bounded_before_api_publication() {
    for (kind, limit) in [("span", 256), ("attribute", 256), ("tag", 512)] {
        let result = command(
            json!({"op":"trace","request":{"op":"label","kind":kind,"value":format!("password=producer-secret-Q7r9T3x6\n{}","é".repeat(1000))}}),
        );
        let text = result["ok"]["result"]["value"]["text"].as_str().unwrap();
        assert_eq!(text.chars().count(), limit);
        assert!(text.starts_with("password") && text.contains("[REDACTED]"));
        assert!(!text.contains("producer-secret"));
    }
}

#[test]
fn trace_usage_keeps_only_nonnegative_integer_observations() {
    let result = command(json!({"op":"trace","request":{"op":"usage","value":{
        "input_tokens":3,"output_tokens":false,"total_tokens":-1,
        "cache_read_input_tokens":2.5,"cache_creation_input_tokens":4,
        "password":"confidential observation","unknown":5}}}));
    assert_eq!(
        result["ok"]["result"]["value"]["value"],
        json!({"input_tokens":3,"cache_creation_input_tokens":4})
    );
}

#[test]
fn small_trace_content_keeps_its_fields_within_the_allocated_budget() {
    let value = json!({"note":"ordinary","messages":["small"]});
    let shaped = command(json!({"op":"shape","input_json":value.to_string()}));
    let shaped = shaped["ok"]["result"]["value"].clone();
    let size = command(json!({"op":"size","shaped":shaped}));
    let size = size["ok"]["result"]["value"].as_u64().unwrap();
    let allocation = command(json!({"op":"trace","request":{"op":"allocate","entries":[
        {"root":true,"kind":"CHAIN","proto_bytes":64,"sizes":[size],"shrunk":false}]}}));
    let limit = allocation["ok"]["result"]["value"]["limits"][0][0]
        .as_u64()
        .unwrap();
    assert!(limit > 0 && limit < 512);
    let cleaned = command(json!({"op":"trace","request":{"op":"clean",
        "contents":[shaped.clone()],"limits":[limit]}}));
    let text = cleaned["ok"]["result"]["value"]["values"][0]
        .as_str()
        .unwrap();
    assert!(text.len() as u64 <= limit);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(text).unwrap(),
        value
    );
    let exhausted = command(json!({"op":"trace","request":{"op":"clean",
        "contents":[shaped],"limits":[0]}}));
    assert_eq!(
        exhausted["ok"]["result"]["value"]["values"][0].as_str(),
        Some(r#"{"truncated": true, "note": "trace content budget exhausted"}"#)
    );
}

#[test]
fn azure_sas_context_redacts_exact_signature_spans_without_granting_generic_sig_semantics() {
    let signature = format!("TL3FAKE{}+/=", "a".repeat(36));
    let encoded = signature
        .replace('+', "%2B")
        .replace('/', "%2F")
        .replace('=', "%3D");
    for (query, raw_signature) in [
        (
            format!("sv=2022-11-02&se=2026-12-01&sr=b&sig={signature}"),
            signature.as_str(),
        ),
        (
            format!("sig={encoded}&ss=b&se=2026-12-01&sv=2022-11-02"),
            encoded.as_str(),
        ),
        (
            format!("%73v=2022-11-02&%73e=2026-12-01&%73r=b&%73ig={encoded}"),
            encoded.as_str(),
        ),
    ] {
        let source = format!("https://st42.blob.core.windows.net/c/b?{query}&after=ordinary");
        let result = command(json!({"op":"text","value":source}));
        let text = result["ok"]["result"]["value"].as_str().unwrap();
        assert!(!text.contains("TL3FAKE"));
        assert!(
            text == source.replace(raw_signature, "[REDACTED]"),
            "SAS public context changed"
        );
        let start = source.find("TL3FAKE").unwrap() + 3;
        let result = command(json!({"op":"window","value":source,"start":start,"end":start+16}));
        assert_eq!(result["ok"]["result"]["value"], "[REDACTED]");
    }
    for source in [
        format!("ordinary sig={encoded}"),
        format!("https://example.com/c?sv=v&se=e&sr=b&sig={encoded}"),
        format!("https://st42.blob.core.windows.net.evil.example/c?sv=v&se=e&sr=b&sig={encoded}"),
        format!("https://st42.blob.core.windows.net/c?sv=v&sr=b&sig={encoded}"),
        format!("https://st42.blob.core.windows.net/c?sv=v&se=&sr=b&sig={encoded}"),
        format!("https://st42.blob.core.windows.net/c#?sv=v&se=e&sr=b&sig={encoded}"),
        format!("https://st42.blob.core.windows.net@evil.example/c?sv=v&se=e&sr=b&sig={encoded}"),
        format!("https://st42.blob.core.windows.net/c?sv=v&sv=w&se=e&sr=b&sig={encoded}"),
    ] {
        let result = command(json!({"op":"text","value":source}));
        let text = result["ok"]["result"]["value"].as_str().unwrap();
        assert!(
            text.contains(&encoded),
            "missing Azure context must retain ordinary weak sig semantics"
        );
    }
}

#[test]
fn azure_sas_rejects_empty_userinfo_but_keeps_at_signs_outside_authority() {
    let signature = format!("TL3FAKE{}%2B%2F%3D", "a".repeat(36));
    let context = format!("sv=v&se=e&sr=b&sig={signature}");
    let source = format!("https://@st42.blob.core.windows.net/c?{context}");
    let result = command(json!({"op":"text","value":source}));
    let text = result["ok"]["result"]["value"].as_str().unwrap();
    assert!(text == source, "empty userinfo acquired SAS semantics");
    let source = format!(
        "https://st42.blob.core.windows.net/path@ordinary?{context}&after=user@example.com"
    );
    let result = command(json!({"op":"text","value":source}));
    let text = result["ok"]["result"]["value"].as_str().unwrap();
    assert!(
        text == source.replace(&signature, "[REDACTED]"),
        "at sign outside authority changed SAS recognition"
    );
}

#[test]
fn trace_clean_and_live_preserve_cleaned_json_text_and_its_measurement() {
    let input = r#"{"z": 18446744073709551616, "a": 1E5, "m": [1.00, -0, 1E999]}"#;
    let shaped = command(json!({"op":"shape","input_json":input}));
    let shaped = shaped["ok"]["result"]["value"].clone();
    let cleaned = command(json!({"op":"clean","shaped":shaped.clone(),"limit":4096}));
    let text = cleaned["ok"]["result"]["value"].as_str().unwrap();
    assert_eq!(text, input);
    let measured = command(json!({"op":"measure_json","input_json":text}));
    let measured = measured["ok"]["result"]["value"].as_u64().unwrap();
    let clean = command(json!({"op":"trace","request":{"op":"clean",
        "contents":[shaped.clone()],"limits":[4096]}}));
    assert_eq!(
        clean["ok"]["result"]["value"]["values"][0].as_str(),
        Some(text)
    );
    let live = command(json!({"op":"trace","request":{"op":"live",
        "enabled":true,"root":false,"spans":0,"used":17,"contents":[shaped]}}));
    let live = &live["ok"]["result"]["value"];
    assert_eq!(live["values"][0].as_str(), Some(text));
    assert_eq!(live["used"], json!(17 + measured));
    assert_eq!(live["spans"], json!(1));
}

#[test]
fn trace_number_validation_preserves_existing_nonfinite_rejection() {
    for atom in ["NaN", "Infinity", "-Infinity"] {
        let result = command(json!({"op":"trace","request":{"op":"clean",
            "contents":[{"kind":"number","value":atom}],"limits":[4096]}}));
        assert!(result.get("error").is_some());
    }
}
