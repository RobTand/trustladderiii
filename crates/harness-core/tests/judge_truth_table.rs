use tl3_harness_core::judge::{Input, Kind, Outcome, Reason, judge};

fn outcome(value: &serde_json::Value) -> Outcome {
    match value.as_str().unwrap() {
        "passed" => Outcome::Passed,
        "assertion" => Outcome::Assertion,
        "added-name" => Outcome::AddedName,
        "error" => Outcome::Error,
        "skipped" => Outcome::Skipped,
        "missing" => Outcome::Missing,
        "uncollected" => Outcome::Uncollected,
        "no-report" => Outcome::NoReport,
        value => panic!("unknown outcome {value}"),
    }
}

#[test]
fn all_780_python_judge_vectors_match_the_real_rust_decision_and_refusals() {
    use serde_json::{Value, json};
    use tl3_harness_core::judge::{Failure, message};
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../harness/test-vectors/core/judge.json");
    let corpus: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 780);
    for case in cases {
        let data = &case["input"];
        let kind = match data["check_kind"].as_str().unwrap() {
            "demonstrating" => Kind::Demonstrating,
            "reproducing" => Kind::Reproducing,
            "regression" => Kind::Regression,
            "behavior-preserving" => Kind::BehaviorPreserving,
            value => panic!("unknown kind {value}"),
        };
        let proof = data["proof"].as_str().unwrap();
        let input = Input {
            kind,
            base: outcome(&data["base"]),
            head: outcome(&data["head"]),
            forced: data.get("forced").map(outcome),
            reason: match proof {
                "absent" => Reason::Undeclared,
                "message" | "source-assertion" => Reason::Matched,
                _ => Reason::Mismatch,
            },
        };
        let decision = judge(input);
        let failure = (proof != "missing-failure").then_some(Failure {
            phase: if proof == "setup" { "setup" } else { "call" },
            exception: "AssertionError",
        });
        let result = json!({
            "name": "vector", "kind": kind.as_str(), "custody": "in-process",
            "verdict": if decision.infrastructure { "infrastructure-failure" }
                else if decision.passed { "pass" } else { "fail" },
            "tests": [{"test": "tests/test_value.py::test_value", "base": input.base.as_str(),
                "head": input.head.as_str(), "head_forced": input.forced.map(Outcome::as_str),
                "pre_existing": decision.pre_existing, "passed": decision.passed, "phases": null,
                "message": message(input, "", "", failure.as_ref(), &[], "expected")}],
            "forcing": input.forced.map(|_| json!({"symptom": "forced", "environment": [["REPRODUCE", "1"]], "patches": []})),
            "proof": null, "selection": null, "declared_symptom": null, "symptom_evidence": null,
        });
        assert_eq!(result, case["expected"], "judge/{}", case["id"]);
    }
    println!(
        "judge: {} complete Python/Rust outputs checked",
        cases.len()
    );
}

#[test]
fn declared_assertion_requires_both_head_runs_and_exact_red() {
    let input = Input {
        kind: Kind::Demonstrating,
        base: Outcome::Assertion,
        head: Outcome::Passed,
        forced: Some(Outcome::Passed),
        reason: Reason::Matched,
    };
    assert!(judge(input).passed);
    assert!(
        !judge(Input {
            reason: Reason::Undeclared,
            ..input
        })
        .passed
    );
    assert!(
        !judge(Input {
            reason: Reason::Mismatch,
            ..input
        })
        .passed
    );
    assert!(
        !judge(Input {
            forced: Some(Outcome::Error),
            ..input
        })
        .passed
    );
    assert!(
        judge(Input {
            head: Outcome::NoReport,
            ..input
        })
        .infrastructure
    );
}

#[test]
fn regression_preserves_base_failures_but_requires_available_base() {
    let input = Input {
        kind: Kind::Regression,
        base: Outcome::Error,
        head: Outcome::Assertion,
        forced: None,
        reason: Reason::Undeclared,
    };
    let decision = judge(input);
    assert!(decision.passed && decision.pre_existing && !decision.infrastructure);
    assert!(
        !judge(Input {
            base: Outcome::Skipped,
            head: Outcome::Passed,
            ..input
        })
        .passed
    );
}

/// #749 regression-added-name-passed: this conflicts with #822's example property.
#[test]
fn regression_added_name_base_and_passing_head_preserve_current_parity() {
    assert!(
        judge(Input {
            kind: Kind::Regression,
            base: Outcome::AddedName,
            head: Outcome::Passed,
            forced: None,
            reason: Reason::Undeclared,
        })
        .passed
    );
}
