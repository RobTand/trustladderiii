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
