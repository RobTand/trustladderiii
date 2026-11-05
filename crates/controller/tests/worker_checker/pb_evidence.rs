//! #821: parity and differential I/O schedules against the original Python shells.
use proptest::{
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

fn drive(script: &str, args: &[String]) -> u64 {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut child = Command::new(root.join("../../workers/.venv/bin/python"))
        .arg("-c")
        .arg(script)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut errors = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut raw = Vec::new();
        errors.read_to_end(&mut raw).unwrap();
        raw
    });
    let mut input = child.stdin.take().unwrap();
    let mut count = 0;
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let line = line.unwrap();
        let value: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
        if let Some(request) = value.get("request") {
            #[derive(serde::Deserialize)]
            struct Envelope {
                request: Box<serde_json::value::RawValue>,
            }
            let original: Envelope = serde_json::from_str(&line).unwrap();
            let response = tl3_pb_evidence::wire::respond(
                request["operation"].as_str().unwrap(),
                original.request.get(),
            )
            .unwrap();
            writeln!(input, "{}", response.get()).unwrap();
            input.flush().unwrap();
        } else {
            count = value["done"].as_u64().unwrap();
        }
    }
    drop(input);
    let status = child.wait().unwrap();
    let errors = errors.join().unwrap();
    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    count
}

/// #821 / #704 / Q55: changing upstream reads and crashes preserve effects and identities.
#[test]
fn wait_io_schedules_preserve_sampling_and_durable_effects() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let strategy = (
        0usize..8,
        0usize..8,
        proptest::bool::ANY,
        proptest::bool::ANY,
        0usize..5,
    );
    let mut runner = TestRunner::deterministic();
    let mut schedules = Vec::new();
    for _ in 0..128 {
        let (waits, kind, base, forcing, crash) = strategy.new_tree(&mut runner).unwrap().current();
        schedules.push(serde_json::json!([
            waits,
            ([
                "valid",
                "wrong_key",
                "unverified",
                "failed",
                "oom",
                "proof_mismatch",
                "forcing_mismatch",
                "symptom_mismatch"
            ][kind]),
            base,
            forcing,
            ([
                None,
                Some("stage"),
                Some("git"),
                Some("checkpoint"),
                Some("log")
            ][crash])
        ]));
    }
    assert_eq!(
        drive(
            include_str!("pb_evidence/wait_schedule.py"),
            &[
                root.join("../../workers/tests/pb-evidence-reference/pb_tests.py")
                    .display()
                    .to_string(),
                serde_json::to_string(&schedules).unwrap()
            ]
        ),
        768
    );
}

/// #821: the importing journal, each output replacement and the applied journal retain their order.
#[test]
fn generator_io_schedules_preserve_journals_and_every_crash_point() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let count = drive(
        include_str!("pb_evidence/generator_schedule.py"),
        &[
            root.join("../../workers/tests/pb-evidence-reference/pb_generators.py")
                .display()
                .to_string(),
            root.join("../../workers/tests/pb-evidence-reference/pb_tests.py")
                .display()
                .to_string(),
        ],
    );
    assert!(
        count >= 60,
        "every generator write must be covered: {count}"
    );
}

/// #821 / Q55: every frozen P3a case executes through both shells; shards cover disjoint cases.
fn corpus(kind: &str, part: usize, parts: usize) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read_to_string(
        root.join("../../workers/tests/pb-evidence-vectors")
            .join(format!("{kind}.json")),
    )
    .unwrap();
    let value: Value = serde_json::from_str(&raw).unwrap();
    let expected = (0..value["cases"].as_array().unwrap().len())
        .filter(|i| i % parts == part)
        .count() as u64;
    assert!(expected > 0);
    assert_eq!(
        drive(
            include_str!("pb_evidence/vector_parity.py"),
            &[
                root.join("tests/worker_checker/pb_evidence")
                    .display()
                    .to_string(),
                kind.into(),
                part.to_string(),
                parts.to_string()
            ]
        ),
        expected
    );
}
macro_rules! corpus_tests {
    ($($name:ident => ($kind:literal,$part:literal,$parts:literal)),* $(,)?) => {$ (
        #[test]
        fn $name() { corpus($kind,$part,$parts); }
    )*};
}
corpus_tests! {
    corpus_ending_summary => ("ending-summary",0,1),
    corpus_failure_receipt => ("failure-receipt",0,1),
    corpus_post_wait_consistency => ("post-wait-consistency",0,1),
    corpus_machine_record_fallback => ("machine-record-fallback",0,1),
    corpus_generator_artifact_0 => ("generator-artifact",0,4),
    corpus_generator_artifact_1 => ("generator-artifact",1,4),
    corpus_generator_artifact_2 => ("generator-artifact",2,4),
    corpus_generator_artifact_3 => ("generator-artifact",3,4),
    corpus_rehearsal_identity => ("rehearsal-identity",0,1),
}

/// #821: float decoding is a boundary concern; Python's exact spelling is identity data.
#[test]
fn generated_serializers_match_python_bytes() {
    let strategy = (
        proptest::num::f64::NORMAL | proptest::num::f64::SUBNORMAL | proptest::num::f64::ZERO,
        ".{0,100}",
        proptest::num::i64::ANY,
    );
    let mut runner = TestRunner::deterministic();
    let mut values = vec!["[-0.0,1.0,1e-7,1e16,0.0001,0.00001]".to_string()];
    for _ in 0..256 {
        let (number, text, integer) = strategy.new_tree(&mut runner).unwrap().current();
        values.push(
            serde_json::to_string(
                &serde_json::json!({"number":number,"text":text,"integer":integer}),
            )
            .unwrap(),
        );
    }
    assert_eq!(
        drive(
            include_str!("pb_evidence/serializer.py"),
            &[serde_json::to_string(&values).unwrap()]
        ),
        1028
    );
}

/// #821: the wire schema is generated from Rust; the checked-in artifact cannot drift.
#[test]
fn pb_evidence_wire_schema_matches_rust_types() {
    assert_eq!(
        serde_json::to_string_pretty(&schemars::schema_for!(tl3_pb_evidence::Request)).unwrap(),
        include_str!("../../../pb-evidence/tests/request-schema.json")
    );
}

/// #821: maximum-size logs finish within the unchanged strict subprocess transport.
#[test]
fn artifact_limit_uses_the_strict_real_binary_transport() {
    let script = r#"
import json,sys
from pathlib import Path
sys.path.insert(0, str(Path(sys.argv[1]) / 'workers/tests'))
import pb_evidence_vectors as oracle
cases = oracle.read_vectors()['generator-artifact']
for case in cases:
    if case['case'] in ('artifact-bytes-below', 'artifact-bytes-at'):
        actual = oracle.observe('generator-artifact', case['input'])
        assert oracle.encoded(actual) == oracle.encoded(case['output']), (case['case'], str(actual)[:500])
print(json.dumps({'done': 2}))
"#;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert_eq!(drive(script, &[root.display().to_string()]), 2);
}
