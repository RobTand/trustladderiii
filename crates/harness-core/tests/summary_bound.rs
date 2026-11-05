use tl3_harness_core::summary::{
    BoundError, PythonEnvelope, SummarySerializer, SummaryValue, bounded,
};

fn value(source: &str) -> SummaryValue {
    SummaryValue::from_python_json(source).unwrap()
}

#[test]
fn python_envelope_preserves_insertion_order_spaces_and_ascii_escapes() {
    let input = value(r#"{"z": ["é😀", "\ud800", "\n\u007f"], "a": true}"#);
    let encoded = PythonEnvelope.serialize(&input).unwrap();
    assert_eq!(
        encoded,
        br#"{"z": ["\u00e9\ud83d\ude00", "\ud800", "\n\u007f"], "a": true}"#,
    );
}

#[test]
fn small_summary_is_preserved_exactly_without_a_notice() {
    let input = value(r#"{"status": "completed", "tests": [], "totals": {}, "note": ""}"#);
    let expected = PythonEnvelope.serialize(&input).unwrap();
    let observed = bounded(input, expected.len(), &PythonEnvelope).unwrap();
    assert_eq!(PythonEnvelope.serialize(&observed.value).unwrap(), expected);
    assert!(!observed.report.diagnostic);
    assert!(observed.report.dropped.is_empty());
}

#[test]
fn mandatory_core_that_fits_cannot_be_replaced_by_a_diagnostic() {
    let input = value(
        r#"{"status":"completed","tests":[{"test":"test_x.py::test_red","outcome":"failed","message":"RED"}],"proof":{"mode":"head-observer"},"totals":{"failed":1},"note":"","origin":"test_run"}"#,
    );
    let observed = bounded(input.clone(), 512, &PythonEnvelope).unwrap();
    assert!(!observed.report.diagnostic);
    assert!(observed.serialized_bytes <= 512);
    assert_eq!(observed.value, input);
}

#[test]
fn oversized_proof_returns_the_origin_preserving_diagnostic() {
    let input = value(&format!(
        r#"{{"status":"completed","tests":[],"totals":{{"failed":1}},"proof":{{"payload":"{}"}},"origin":"test_run","note":""}}"#,
        "x".repeat(20_000)
    ));
    let observed = bounded(input, 16_384, &PythonEnvelope).unwrap();
    assert!(observed.report.diagnostic);
    let wire = String::from_utf8(PythonEnvelope.serialize(&observed.value).unwrap()).unwrap();
    assert!(wire.contains("identity_exceeds_summary_limit"));
    assert!(wire.contains(r#""origin": "test_run""#));
    assert!(wire.len() <= 16_384);
}

#[test]
fn a_limit_smaller_than_the_diagnostic_returns_a_typed_error() {
    let input = value(
        r#"{"status":"completed","tests":[],"totals":{},"proof":{"required":"data"},"note":""}"#,
    );
    assert!(matches!(
        bounded(input, 0, &PythonEnvelope),
        Err(BoundError::DiagnosticTooLarge { limit: 0, .. }),
    ));
}

#[test]
fn seven_groups_keep_proof_totals_and_red_when_any_display_survives() {
    let group = |index: usize| {
        format!(r#"{{"status":"completed","check":"check-{index}","proof":{{"mode":"head-observer","source_base_tree_sha256":"{}"}},"totals":{{"failed":1,"error":30}},"tests":[{{"test":"test_x.py::test_red_{index}","outcome":"failed","message":"RED"}},{}],"note":""}}"#, "a".repeat(64), (0..30).map(|test| format!(r#"{{"test":"test_x.py::test_error_{index}_{test}","outcome":"error","message":"{}"}}"#, "x".repeat(400))).collect::<Vec<_>>().join(","))
    };
    let input = value(&format!(
        r#"{{"status":"completed","groups":[{}],"tests":[],"totals":{{"failed":7,"error":210}},"note":""}}"#,
        (0..7).map(group).collect::<Vec<_>>().join(",")
    ));
    let observed = bounded(input, 16_384, &PythonEnvelope).unwrap();
    assert!(!observed.report.diagnostic);
    let wire = String::from_utf8(PythonEnvelope.serialize(&observed.value).unwrap()).unwrap();
    assert!(wire.len() <= 16_384);
    for index in 0..7 {
        assert!(wire.contains(&format!("test_x.py::test_red_{index}")));
    }
    assert_eq!(wire.matches("source_base_tree_sha256").count(), 7);
}

fn fixture(name: &str) -> serde_json::Value {
    let relative = format!("harness/test-vectors/{name}.json");
    let path = [
        std::path::PathBuf::from(&relative),
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(&relative),
    ]
    .into_iter()
    .find(|path| path.exists())
    .expect("shared Python vectors staged");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn vector_value(value: &serde_json::Value) -> SummaryValue {
    use tl3_harness_core::summary::{JsonNumber, PythonText};
    match value {
        serde_json::Value::Null => SummaryValue::Null,
        serde_json::Value::Bool(value) => SummaryValue::Bool(*value),
        serde_json::Value::Number(value) => {
            SummaryValue::Number(JsonNumber::parse(&value.to_string()).unwrap())
        }
        serde_json::Value::String(value) => SummaryValue::String(PythonText::from(value.as_str())),
        serde_json::Value::Array(values) => {
            SummaryValue::Array(values.iter().map(vector_value).collect())
        }
        serde_json::Value::Object(fields)
            if fields.len() == 1 && fields.contains_key("python_codepoints") =>
        {
            SummaryValue::String(
                PythonText::try_from(
                    fields["python_codepoints"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|n| n.as_u64().unwrap() as u32)
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            )
        }
        serde_json::Value::Object(fields) => SummaryValue::Object(
            fields
                .iter()
                .map(|(name, value)| (PythonText::from(name.as_str()), vector_value(value)))
                .collect(),
        ),
    }
}

#[test]
fn shared_bounding_vectors_and_both_incident_properties() {
    let corpus = fixture("core/summary-bound");
    let mut bounds = 0;
    for case in corpus["cases"].as_array().unwrap() {
        let data = &case["input"];
        if data.get("result").is_none() {
            // These pin the unchanged pytest producer's clipping/admission or
            // exception description. Re-serialize its exact Python fact value.
            if let Some(expected) = case["expected"].get("result") {
                let bytes = PythonEnvelope.serialize(&vector_value(expected)).unwrap();
                assert_eq!(
                    bytes.len() as u64,
                    case["expected"]["json_bytes"].as_u64().unwrap(),
                    "{}",
                    case["id"]
                );
            }
            continue;
        }
        bounds += 1;
        let input = vector_value(&data["result"]);
        let limit = data["limit"].as_u64().unwrap_or(14_336) as usize;
        let output = bounded(input.clone(), limit, &PythonEnvelope).unwrap();
        let bytes = PythonEnvelope.serialize(&output.value).unwrap();
        assert!(bytes.len() <= limit, "{}", case["id"]);
        if case.get("divergence").is_none() {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                case["expected"]["result"],
                "{}",
                case["id"]
            );
        } else if case["id"] == "demonstrating-red-before-unplanned-errors" {
            assert!(!output.report.diagnostic);
            let red = data["red_test"].as_str().unwrap();
            let SummaryValue::Array(groups) = output.value.field("groups").unwrap() else {
                unreachable!()
            };
            let SummaryValue::Array(original) = input.field("groups").unwrap() else {
                unreachable!()
            };
            let SummaryValue::Array(tests) = groups[0].field("tests").unwrap() else {
                unreachable!()
            };
            assert!(
                tests
                    .iter()
                    .any(|entry| entry.field("test") == Some(&SummaryValue::String(red.into())))
            );
            for (retained, before) in groups.iter().zip(original) {
                assert_eq!(retained.field("proof"), before.field("proof"));
                assert_eq!(retained.field("totals"), before.field("totals"));
            }
            let SummaryValue::Array(flat) = output.value.field("tests").unwrap() else {
                unreachable!()
            };
            assert!(
                flat.iter()
                    .any(|entry| entry.field("test") == Some(&SummaryValue::String(red.into())))
            );
            assert_eq!(output.value.field("totals"), input.field("totals"));
        } else if case["id"] == "grouped-base-48-checks" {
            assert!(
                output.report.diagnostic,
                "48 mandatory group identities exceed this envelope"
            );
            assert_eq!(output.value.field("totals"), input.field("totals"));
            assert_eq!(output.value.field("origin"), input.field("origin"));
        }
    }
    assert!(
        bounds >= 3,
        "must execute real bounding, including both divergences"
    );
}

#[test]
fn every_python_envelope_atom_is_byte_exact() {
    use tl3_harness_core::summary::PythonText;
    let rows = fixture("summary/python-envelope");
    for row in rows.as_array().unwrap() {
        let points = row["codepoints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_u64().unwrap() as u32)
            .collect::<Vec<_>>();
        let text = SummaryValue::String(PythonText::try_from(points).unwrap());
        assert_eq!(
            PythonEnvelope.serialize(&text).unwrap(),
            row["wire"].as_str().unwrap().as_bytes()
        );
    }
}

proptest::proptest! {
    /// #823: compose arbitrary Python-oracle atoms, including isolated surrogates.
    #[test]
    fn python_envelope_matches_composed_python_vectors(indices in proptest::collection::vec(0usize..516, 0..40)) {
        use tl3_harness_core::summary::PythonText;
        let corpus = fixture("summary/python-envelope");
        let rows = corpus.as_array().unwrap();
        let selected = indices.iter().map(|index| &rows[index % rows.len()]).collect::<Vec<_>>();
        let values = selected.iter().map(|row| SummaryValue::String(PythonText::try_from(row["codepoints"].as_array().unwrap().iter().map(|n| n.as_u64().unwrap() as u32).collect::<Vec<_>>()).unwrap())).collect();
        let expected = format!("[{}]", selected.iter().map(|row| row["wire"].as_str().unwrap()).collect::<Vec<_>>().join(", "));
        proptest::prop_assert_eq!(PythonEnvelope.serialize(&SummaryValue::Array(values)).unwrap(), expected.into_bytes());
    }

    /// #823: generated optional entries cannot remove the mandatory RED/proof/totals.
    #[test]
    fn bounding_conforms_to_the_mandatory_core(count in 0usize..70, width in 0usize..600, limit in 500usize..20_000) {
        let display = (0..count).map(|index| format!(r#",{{"test":"display-{index}","outcome":"error","message":"{}"}}"#, "x".repeat(width))).collect::<String>();
        let source = format!(r#"{{"status":"completed","groups":[{{"proof":{{"mode":"head-observer"}},"totals":{{"failed":1}},"tests":[{{"test":"RED","outcome":"failed","message":"required"}}],"note":""}},{{"tests":[{{"test":"optional","outcome":"passed","message":""}}{display}],"totals":{{"error":{count}}},"note":""}}],"tests":[],"totals":{{"failed":1,"error":{count}}},"note":""}}"#);
        let input = value(&source);
        let result = bounded(input.clone(), limit, &PythonEnvelope);
        if let Ok(result) = result {
            let bytes = PythonEnvelope.serialize(&result.value).unwrap();
            proptest::prop_assert!(bytes.len() <= limit);
            if !result.report.diagnostic {
                proptest::prop_assert!(String::from_utf8(bytes).unwrap().contains("RED"));
                proptest::prop_assert_eq!(result.value.field("totals"), input.field("totals"));
            }
        }
    }
}

proptest::proptest! {
    /// #823: conformance to the Verus-proven core/diagnostic reference decision.
    #[test]
    fn diagnostic_eligibility_is_exactly_the_mandatory_core(
        proof_bytes in 0usize..22_000, red_bytes in 0usize..22_000,
        optional in 0usize..25, limit in 300usize..30_000,
    ) {
        let red = format!(r#"{{"test":"RED","outcome":"failed","message":"{}"}}"#, "r".repeat(red_bytes));
        let core_source = format!(r#"{{"status":"completed","tests":[{red}],"totals":{{"failed":1,"passed":{optional}}},"proof":{{"mode":"head-observer","payload":"{}"}},"note":""}}"#, "p".repeat(proof_bytes));
        let optional_entries = (0..optional).map(|index| format!(r#",{{"test":"display-{index}","outcome":"passed","message":"{}"}}"#, "x".repeat(500))).collect::<String>();
        let input = value(&core_source.replace(&format!("[{red}]"), &format!("[{red}{optional_entries}]")));
        let core = value(&core_source);
        let core_size = PythonEnvelope.serialize(&core).unwrap().len();
        let full_size = PythonEnvelope.serialize(&input).unwrap().len();
        let reference = tl3_harness_core::summary::retention_level(full_size as u64, core_size as u64, limit as u64);
        match bounded(input.clone(), limit, &PythonEnvelope) {
            Ok(result) => {
                proptest::prop_assert_eq!(result.report.diagnostic, reference == 2);
                proptest::prop_assert!(result.serialized_bytes <= limit);
                if !result.report.diagnostic {
                    proptest::prop_assert_eq!(result.value.field("proof"), core.field("proof"));
                    proptest::prop_assert_eq!(result.value.field("totals"), core.field("totals"));
                    let SummaryValue::Array(entries) = result.value.field("tests").unwrap() else { unreachable!() };
                    let SummaryValue::Array(required) = core.field("tests").unwrap() else { unreachable!() };
                    proptest::prop_assert!(entries.contains(&required[0]));
                }
            }
            Err(BoundError::DiagnosticTooLarge { .. }) => proptest::prop_assert_eq!(reference, 2),
            Err(error) => proptest::prop_assert!(false, "unexpected error: {error}"),
        }
    }
}

proptest::proptest! {
    /// #823: Python's canonical numeric atoms include integers beyond f64/u64.
    #[test]
    fn python_numeric_vectors_round_trip_byte_for_byte(indices in proptest::collection::vec(0usize..13, 0..40)) {
        let corpus = fixture("summary/python-numbers");
        let rows = corpus.as_array().unwrap();
        let expected = format!("[{}]", indices.iter().map(|index| rows[*index].as_str().unwrap()).collect::<Vec<_>>().join(", "));
        let input = SummaryValue::from_python_json(&expected).unwrap();
        proptest::prop_assert_eq!(PythonEnvelope.serialize(&input).unwrap(), expected.into_bytes());
    }
}

#[test]
fn an_action_cap_of_small_entries_uses_the_same_removal_order() {
    let tests = (0..8_000)
        .map(|index| format!(r#"{{"test":"entry-{index}","outcome":"passed","message":""}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let input = value(&format!(
        r#"{{"status":"completed","tests":[{tests}],"totals":{{"passed":8000}},"note":""}}"#
    ));
    let result = bounded(input, 16_384, &PythonEnvelope).unwrap();
    assert!(!result.report.diagnostic);
    assert!(result.serialized_bytes <= 16_384);
    let SummaryValue::Array(entries) = result.value.field("tests").unwrap() else {
        unreachable!()
    };
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(
            entry.field("test"),
            Some(&SummaryValue::String(
                format!("entry-{index}").as_str().into()
            ))
        );
    }
}

#[path = "summary_bound/reference.rs"]
mod reference;

/// #823 review: the executable reference must reject the oversized fast path.
#[test]
fn reduction_reference_selects_a_fitting_document_and_retains_mandatory_atoms() {
    assert_eq!(reference::reduce(900, 100, 300, &[980, 700, 200], 400),
        reference::Plan { kind: 1, removed: 2, bytes: 200 });
    assert_eq!(reference::reduce(900, 100, 300, &[980, 700, 200], 100),
        reference::Plan { kind: 2, removed: 3, bytes: 100 });
    assert_eq!(reference::reduce(900, 500, 300, &[980, 700, 600], 400),
        reference::Plan { kind: 3, removed: 0, bytes: 300 });
}
