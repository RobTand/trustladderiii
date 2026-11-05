use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    kind: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    input: Value,
    expected: Value,
    divergence: Option<Divergence>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Divergence {
    reason: String,
    property: String,
}

// Python strings can contain surrogate code points, which Rust strings cannot.
// Keep their integer representation; do not replace or merge surrogate pairs.
fn python_strings(value: &Value, wrappers: &mut usize) {
    match value {
        Value::Object(object) if object.contains_key("python_codepoints") => {
            assert_eq!(object.len(), 1);
            let points = object["python_codepoints"].as_array().unwrap();
            assert!(
                points
                    .iter()
                    .all(|point| point.as_u64().is_some_and(|n| n <= 0x10ffff))
            );
            assert!(
                points
                    .iter()
                    .any(|point| (0xd800..=0xdfff).contains(&point.as_u64().unwrap()))
            );
            *wrappers += 1;
        }
        Value::Object(object) => {
            for value in object.values() {
                python_strings(value, wrappers);
            }
        }
        Value::Array(array) => {
            for value in array {
                python_strings(value, wrappers);
            }
        }
        _ => {}
    }
}

#[test]
fn all_eight_core_kinds_parse_including_python_codepoints() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../harness/test-vectors/core");
    let kinds = [
        "node-id",
        "selector-identity",
        "summary-bound",
        "red-match",
        "limits",
        "judge",
        "phase-fold",
        "finish-teardown",
    ];
    let files: BTreeSet<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(
        files,
        kinds.iter().map(|kind| format!("{kind}.json")).collect()
    );
    let mut wrappers = 0;
    let mut divergences = 0;
    for kind in kinds {
        let raw = std::fs::read_to_string(root.join(format!("{kind}.json"))).unwrap();
        let corpus: Corpus =
            serde_json::from_str(&raw).unwrap_or_else(|error| panic!("{kind}: {error}"));
        assert_eq!(corpus.kind, kind);
        assert!(!corpus.cases.is_empty(), "{kind}");
        let mut ids = BTreeSet::new();
        for case in &corpus.cases {
            assert!(
                !case.id.is_empty() && ids.insert(&case.id),
                "{kind}/{}",
                case.id
            );
            python_strings(&case.input, &mut wrappers);
            python_strings(&case.expected, &mut wrappers);
            if let Some(divergence) = &case.divergence {
                assert_eq!(kind, "summary-bound");
                assert!(!divergence.reason.is_empty() && !divergence.property.is_empty());
                divergences += 1;
            }
        }
        println!("{kind}: {} cases parsed", corpus.cases.len());
    }
    assert!(
        wrappers > 0,
        "the Python-codepoint wrapper must be exercised"
    );
    assert_eq!(divergences, 2);
}
