use super::decide;
use serde_json::Value;

/// #704/#706/#719/#723: compare the Rust boundary with the unedited Python.
#[test]
fn pb_ending_matches_python_corruption_corpus() {
    let directory = tempfile::tempdir().unwrap();
    let reference = directory.path().join("pb_tests_before.py");
    std::fs::write(&reference, include_str!("tests/pb_tests_before.py")).unwrap();
    let output = std::process::Command::new(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../workers/.venv/bin/python"),
    )
    .arg("-c")
    .arg(include_str!("tests/corpus.py"))
    .arg(reference)
    .output()
    .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let cases: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert!(cases.len() > 150);
    for case in cases {
        assert_eq!(decide(case["request"].clone()), case["expected"], "{}", case["name"]);
    }
}
