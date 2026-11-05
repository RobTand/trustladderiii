use tl3_harness_core::phase::{Fact, Fold, Phase, Status, checking, summary_step};

#[test]
fn all_66_python_phase_vectors_match_the_real_rust_fold() {
    use serde_json::{Value, json};
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../harness/test-vectors/core/phase-fold.json");
    let corpus: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 66);
    for case in cases {
        let mut phases = case["input"]["phases"].as_array().unwrap().clone();
        if case["input"]["parallel"] == true {
            phases.push(json!([
                case["input"]
                    .get("when")
                    .and_then(Value::as_str)
                    .unwrap_or("teardown"),
                "passed",
                "",
                false,
                false
            ]));
        }
        let mut folded = Fold::Passed;
        let mut messages = vec![];
        let mut truncated = false;
        for phase in phases {
            let status = match phase[1].as_str().unwrap() {
                "passed" => Status::Passed,
                "failed" => Status::Failed,
                "skipped" => Status::Skipped,
                value => panic!("unknown status {value}"),
            };
            let fact = Fact {
                phase: match phase[0].as_str().unwrap() {
                    "setup" => Phase::Setup,
                    "call" => Phase::Call,
                    "teardown" => Phase::Teardown,
                    _ => Phase::WorkerCrash,
                },
                status,
                assertion: phase[4].as_bool().unwrap(),
            };
            folded = summary_step(folded, fact);
            if status == Status::Failed {
                messages.push(phase[2].as_str().unwrap().to_owned());
                truncated |= phase[3].as_bool().unwrap();
            }
        }
        let message = messages.join("\n");
        truncated |= message.chars().count() > 1000;
        let actual = json!([{"event": "result", "test": "tests/test_value.py::test_value",
            "outcome": match folded { Fold::Passed => "passed", Fold::Failed => "failed",
                Fold::Error => "error", Fold::Skipped => "skipped" },
            "message": message.chars().take(1000).collect::<String>(), "truncated": truncated}]);
        assert_eq!(actual, case["expected"], "phase-fold/{}", case["id"]);
    }
    println!(
        "phase-fold: {} complete Python/Rust outputs checked",
        cases.len()
    );
}

#[test]
fn summary_and_checking_preserve_their_skip_precedence() {
    let call = Fact {
        phase: Phase::Call,
        status: Status::Failed,
        assertion: true,
    };
    let skip = Fact {
        phase: Phase::Teardown,
        status: Status::Skipped,
        assertion: false,
    };
    assert_eq!(
        summary_step(summary_step(Fold::Passed, call), skip),
        Fold::Failed
    );
    assert_eq!(
        checking(Status::Passed, Some(call), Status::Skipped),
        Some(Fold::Skipped)
    );
    assert_eq!(
        checking(Status::Failed, Some(call), Status::Passed),
        Some(Fold::Error)
    );
    assert_eq!(checking(Status::Passed, None, Status::Passed), None);
}

#[test]
fn worker_crash_is_an_error_even_if_it_claims_an_assertion() {
    let crash = Fact {
        phase: Phase::WorkerCrash,
        status: Status::Failed,
        assertion: true,
    };
    assert_eq!(summary_step(Fold::Passed, crash), Fold::Error);
}
