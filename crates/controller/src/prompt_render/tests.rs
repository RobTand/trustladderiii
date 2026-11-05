use super::render;
use proptest::prelude::*;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
struct Vector {
    kind: String,
    document: String,
    expected: String,
}

fn vectors() -> Vec<Vector> {
    serde_json::from_str(include_str!("vectors.json")).unwrap()
}

/// #819: system text, shared prefixes and cache identities must remain byte-identical.
#[test]
fn python_prompt_identity_vectors() {
    let rows = vectors();
    assert!(rows.len() > 100);
    for row in rows {
        let actual = render(&row.kind, &row.document).unwrap();
        assert_eq!(actual, row.expected, "{row:?}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]
    /// #819: compose real-Python number/string cases under randomly ordered nested objects.
    #[test]
    fn python_serializer_corpus_composes(indices in prop::collection::vec(0usize..512, 0..24), reverse in any::<bool>(), depth in 0usize..8) {
        let rows: Vec<_> = vectors().into_iter().filter(|v| v.kind == "serialize" && serde_json::from_str::<serde_json::Value>(&v.expected).is_ok()).collect();
        let selected: Vec<_> = indices.iter().map(|i| &rows[i % rows.len()]).collect();
        let mut document = format!("[{}]", selected.iter().map(|r| r.document.as_str()).collect::<Vec<_>>().join(","));
        let mut expected = format!("[{}]", selected.iter().map(|r| serde_json::from_str::<serde_json::Value>(&r.expected).unwrap()["text"].as_str().unwrap().to_owned()).collect::<Vec<_>>().join(", "));
        for level in 0..depth {
            document = if reverse ^ (level % 2 == 0) {
                format!(r#"{{"😀":{document},"a":{{"z":0,"é":true}}}}"#)
            } else {
                format!(r#"{{"a":{{"é":true,"z":0}},"😀":{document}}}"#)
            };
            expected = format!(r#"{{"a": {{"z": 0, "é": true}}, "😀": {expected}}}"#);
        }
        let actual = render("serialize", &document).unwrap();
        let actual: serde_json::Value = serde_json::from_str(&actual).unwrap();
        prop_assert_eq!(actual["text"].as_str().unwrap(), expected);
    }
}

/// #819: exercise the real worker transport, Python fallback and worker-start reads.
#[test]
fn worker_sampling_and_transport_match_pre_extraction() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    use proptest::strategy::ValueTree;
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let schedules = proptest::collection::vec((0u8..3, 0u8..10), 64)
        .new_tree(&mut runner)
        .unwrap()
        .current();
    let output = std::process::Command::new(root.join("workers/.venv/bin/python"))
        .arg("-c")
        .arg(include_str!("worker_parity.py"))
        .arg(serde_json::to_string(&schedules).unwrap())
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{}", String::from_utf8_lossy(&output.stdout));
}

/// #819: malformed render calls fail closed before reading any runtime config.
#[test]
fn renderer_refuses_duplicate_keys_and_malformed_json() {
    for input in [
        r#"{"role":"ingest","role":"planner"}"#,
        "[1,]",
        "{\"x\":1,}",
        "1e+",
        "01",
        "\"\\q\"",
        "{} trailing",
    ] {
        assert!(render("serialize", input).is_err(), "{input}");
    }
}

/// #819: witnesses reach every prefix variant and every shared-role classification.
#[test]
fn prompt_policy_reachability_witnesses() {
    use super::decision::{Prefix, Role, prefix, shared};
    for role in [
        Role::Ingest,
        Role::Audit,
        Role::Planner,
        Role::Maker,
        Role::Reviewer,
        Role::Classifier,
        Role::Writer,
        Role::Postmortem,
    ] {
        let expected = matches!(role, Role::Ingest | Role::Audit | Role::Planner);
        assert_eq!(shared(role), expected);
        assert_eq!(prefix(role, false, true), Prefix::None);
        assert_eq!(
            prefix(role, true, false),
            if expected {
                Prefix::Plain
            } else {
                Prefix::None
            }
        );
        assert_eq!(
            prefix(role, true, true),
            if expected {
                Prefix::Ephemeral
            } else {
                Prefix::None
            }
        );
    }
}
