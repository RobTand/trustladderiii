//! #819: pin the pre-existing pattern divergence; this migration changes neither side.

use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

#[derive(Deserialize)]
struct Vectors {
    schema: String,
    pointer: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    value: Value,
    rust_valid: bool,
    python_valid: bool,
}

#[test]
fn preexisting_pattern_divergence_is_preserved() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../harness/test-vectors/contract-patterns.json");
    let vectors: Vectors = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(vectors.schema, "request");
    assert_eq!(vectors.cases.len(), 3);
    assert!(
        vectors
            .cases
            .iter()
            .any(|case| case.rust_valid != case.python_valid)
    );
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../workers/src/tl3_workers/schemas/request.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(schema.pointer(&vectors.pointer).unwrap()).unwrap();
    for case in vectors.cases {
        assert_eq!(
            validator.is_valid(&case.value),
            case.rust_valid,
            "{}",
            case.name
        );
    }
}
