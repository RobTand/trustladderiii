//! #821 / Q55: the frozen Python corpus constrains the extraction.
use serde_json::Value;

fn parity(kind: &str) {
    let raw = match kind {
        "ending-summary" => include_str!("ending-summary.json"),
        "failure-receipt" => include_str!("failure-receipt.json"),
        "post-wait-consistency" => include_str!("post-wait-consistency.json"),
        "machine-record-fallback" => include_str!("machine-record-fallback.json"),
        "generator-artifact" => include_str!("generator-artifact.json"),
        "rehearsal-identity" => include_str!("rehearsal-identity.json"),
        _ => unreachable!(),
    };
    let document: Value = serde_json::from_str(raw).unwrap();
    let cases = document["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let actual = tl3_pb_evidence::decide(kind, case["input"].clone());
        assert_eq!(actual, case["output"], "{kind}/{}", case["case"]);
    }
}

#[test]
fn ending_summary() {
    parity("ending-summary");
}
#[test]
fn failure_receipt() {
    parity("failure-receipt");
}
#[test]
fn post_wait_consistency() {
    parity("post-wait-consistency");
}
#[test]
fn machine_record_fallback() {
    parity("machine-record-fallback");
}
#[test]
fn generator_artifact() {
    parity("generator-artifact");
}
#[test]
fn rehearsal_identity() {
    parity("rehearsal-identity");
}
