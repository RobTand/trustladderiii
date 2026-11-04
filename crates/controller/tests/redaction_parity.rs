mod redaction_support;
use redaction_support::{python_command, python_redact, shared_inputs};

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

/// #753 re-review: folding must preserve the literal trailing hyphen in BEARER's class.
#[test]
fn bearer_hyphens_redact_identically() {
    let inputs = [
        "Basic zE9p-R4cV7x",
        "Basıc zE9p-R4cV7x",
        "Basİc zE9pR4cV7x-tail",
        "Basic zE9pR4cV7x-tail",
    ]
    .map(str::to_owned);
    for (input, expected) in inputs.iter().zip(python_redact(&inputs)) {
        assert_eq!(
            expected,
            format!("{} [REDACTED]", input.split_once(' ').unwrap().0)
        );
        assert_eq!(
            tl3_controller::redaction::text(input).unwrap(),
            expected,
            "{input:?}"
        );
    }
}
