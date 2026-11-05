//! #821: parity and differential I/O schedules against the original Python shells.
use proptest::{
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
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
    let mut input = child.stdin.take().unwrap();
    let mut count = 0;
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let line = line.unwrap();
        let value: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
        if let Some(request) = value.get("request") {
            let request: tl3_pb_evidence::Request =
                serde_json::from_value(request.clone()).unwrap();
            let response = match tl3_pb_evidence::evaluate(request) {
                Ok(v) => tl3_pb_evidence::Response::Return(v),
                Err(e) => tl3_pb_evidence::Response::Raise(e),
            };
            writeln!(input, "{}", serde_json::to_string(&response).unwrap()).unwrap();
            input.flush().unwrap();
        } else {
            count = value["done"].as_u64().unwrap();
        }
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
                root.join("tests/pb_evidence/pb_tests_before.py")
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
            root.join("tests/pb_evidence/pb_generators_before.py")
                .display()
                .to_string(),
            root.join("tests/pb_evidence/pb_tests_before.py")
                .display()
                .to_string(),
        ],
    );
    assert!(
        count >= 60,
        "every generator write must be covered: {count}"
    );
}

/// #821 / Q55: every frozen P3a vector executes through both original and extracted shells.
#[test]
fn complete_python_corpus_matches_the_rust_worker() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(drive(include_str!("pb_evidence/vector_parity.py"), &[
        root.join("tests/pb_evidence").display().to_string()
    ]), 252);
}

/// #821: float decoding is a boundary concern; Python's exact spelling is identity data.
#[test]
fn generated_serializers_match_python_bytes() {
    let strategy = (proptest::num::f64::NORMAL | proptest::num::f64::SUBNORMAL | proptest::num::f64::ZERO,
        ".{0,100}", proptest::num::i64::ANY);
    let mut runner = TestRunner::deterministic();
    let mut values = vec!["[-0.0,1.0,1e-7,1e16,0.0001,0.00001]".to_string()];
    for _ in 0..256 {
        let (number,text,integer) = strategy.new_tree(&mut runner).unwrap().current();
        values.push(serde_json::to_string(&serde_json::json!({"number":number,"text":text,"integer":integer})).unwrap());
    }
    assert_eq!(drive(include_str!("pb_evidence/serializer.py"), &[
        serde_json::to_string(&values).unwrap()
    ]),1028);
}
