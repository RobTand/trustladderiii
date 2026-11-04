use std::path::Path;
use std::process::Command;

fn python_command() -> Command {
    let mut command =
        Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workers/.venv/bin/python"));
    command.env_clear();
    for key in [
        "PATH",
        "HOME",
        "TMPDIR",
        "TEST_TMPDIR",
        "TEST_SRCDIR",
        "TEST_WORKSPACE",
        "RUNFILES_DIR",
        "RUNFILES_MANIFEST_FILE",
        "JAVA_RUNFILES",
        "PYTHON_RUNFILES",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
}

fn shared_inputs() -> Vec<String> {
    let mut matrix: Vec<String> =
        serde_json::from_str(include_str!("../src/redaction/matrix.json")).unwrap();
    assert_eq!(matrix.len(), 196);
    matrix.extend(
        serde_json::from_str::<Vec<String>>(include_str!("../src/redaction/corpus.json")).unwrap(),
    );
    matrix
}

fn python_redact(matrix: &[String]) -> Vec<String> {
    use std::io::Write;
    let mut child = python_command()
            .args(["-I", "-c", "import json,sys; from tl3_workers.tracing import Redactor; r=Redactor(None); print(json.dumps([r.text(s,limit=len(s)) for s in json.load(sys.stdin)]))"])
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
            .spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(matrix).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let expected: Vec<String> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(expected.len(), matrix.len());
    expected
}

#[test]
fn matches_python_on_the_shared_matrix() {
    let matrix = shared_inputs();
    let expected = python_redact(&matrix);
    for (input, expected) in matrix.iter().zip(expected) {
        assert_eq!(
            tl3_controller::redaction::text(input).unwrap(),
            expected,
            "{input:?}"
        );
    }
}

#[test]
fn matches_python_on_all_planted_forms() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workers/tests");
    let output = python_command().args(["-I", "-c",
            "import json,sys; sys.path.insert(0,sys.argv[1]); from redaction_branches import original_inputs,branch_inputs; print(json.dumps(original_inputs()+branch_inputs()))"])
            .arg(directory).output().unwrap();
    assert!(output.status.success());
    let inputs: Vec<String> = serde_json::from_slice(&output.stdout).unwrap();
    assert!(inputs.len() >= 400);
    for (input, expected) in inputs.iter().zip(python_redact(&inputs)) {
        assert_eq!(
            tl3_controller::redaction::text(input).unwrap(),
            expected,
            "{input:?}"
        );
    }
}

/// #753 security review: Python's Turkish I folding must also protect controller output.
#[test]
fn turkish_scheme_redacts_identically() {
    let inputs = vec!["Basıc zE9pR4cV7x".to_owned(), "Basİc zE9pR4cV7x".to_owned()];
    for (input, expected) in inputs.iter().zip(python_redact(&inputs)) {
        assert!(expected.contains("[REDACTED]"));
        assert_eq!(tl3_controller::redaction::text(input).unwrap(), expected);
    }
}

#[test]
/// Issue #753, wave G0: corpus-derived spans retain exact Rust/Python redaction parity.
fn corpus_seeded_proptest_matches_python() {
    use proptest::{
        prelude::*,
        test_runner::{Config, RngSeed, TestRunner},
    };
    let corpus = shared_inputs();
    let mut runner = TestRunner::new(Config {
        cases: 1,
        max_shrink_iters: 64,
        rng_seed: RngSeed::Fixed(753),
        failure_persistence: None,
        ..Config::default()
    });
    // One property round compares 64 generated spans through one Python boundary call.
    // Shrinking retains the same parity predicate over a smaller-valued batch.
    let strategy = prop::collection::vec((0..corpus.len(), 0usize..4, 0usize..4), 64);
    runner
        .run(&strategy, |cases| {
            let inputs = cases
                .into_iter()
                .map(|(index, depth, padding)| {
                    let mut input = corpus[index].clone();
                    for _ in 0..depth {
                        input = serde_json::to_string(&input).unwrap();
                    }
                    format!("{}{}{}", "\n".repeat(padding), input, "\n".repeat(padding))
                })
                .collect::<Vec<_>>();
            let expected = python_redact(&inputs);
            for (input, expected) in inputs.iter().zip(expected) {
                prop_assert_eq!(tl3_controller::redaction::text(input).unwrap(), expected);
            }
            Ok(())
        })
        .unwrap();
}
