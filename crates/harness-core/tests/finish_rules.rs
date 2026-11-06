use tl3_harness_core::finish::{Mode, consistent, process_exit};

#[test]
fn python_finish_teardown_vectors_match_the_real_rust_mapping() {
    use serde_json::Value;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../harness/test-vectors/core/finish-teardown.json");
    let corpus: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let data = &case["input"];
        let mode = if data["mode"] == "checking" {
            Mode::Checking
        } else {
            Mode::Summary
        };
        if data["op"] == "run-pytest" {
            let mapped = process_exit(
                data["return_code"].as_i64().unwrap_or(0) as i32,
                data["finished"].as_i64().map(|code| code as i32),
                data["raised"].as_bool().unwrap(),
                data["worker_teardown_exhausted"].as_bool().unwrap(),
            );
            assert_eq!(
                Value::from(mapped),
                case["expected"],
                "finish-teardown/{}",
                case["id"]
            );
        } else {
            let exit = data["exit_code"].as_i64().unwrap() as i32;
            let finished = match data["finish"].as_str().unwrap() {
                "matching" => Some(exit),
                "missing" => None,
                "mismatched" => Some((exit + 1) % 6),
                value => panic!("unknown finish {value}"),
            };
            let accepted = consistent(mode, finished, exit)
                && match mode {
                    Mode::Checking => tl3_harness_core::finish::checking_outcomes(
                        exit,
                        exit == 0,
                        matches!(exit, 1..=3),
                    ),
                    Mode::Summary => tl3_harness_core::finish::summary_outcomes(
                        exit,
                        exit == 2,
                        matches!(exit, 1 | 3),
                    ),
                };
            let expected_accepted = match mode {
                Mode::Checking => case["expected"].get("value").is_some(),
                Mode::Summary => case["expected"]["status"] == "completed",
            };
            assert_eq!(
                accepted, expected_accepted,
                "finish-teardown/{}",
                case["id"]
            );
        }
    }
    println!(
        "finish-teardown: {} Python/Rust mappings checked",
        cases.len()
    );
}

#[test]
fn only_xdist_teardown_thread_failure_preserves_finish() {
    assert_eq!(process_exit(0, Some(1), true, true), 1);
    assert_eq!(process_exit(0, Some(1), true, false), 3);
    assert_eq!(process_exit(0, None, true, true), 3);
    assert_eq!(process_exit(1, Some(0), false, false), 1);
}

#[test]
fn internal_error_is_refused_in_every_mode() {
    for mode in [Mode::Checking, Mode::Summary] {
        assert!(!consistent(mode, Some(3), 3));
        assert!(!consistent(mode, Some(1), 0));
        assert!(!consistent(mode, None, 0));
    }
}

#[test]
fn mode_specific_consistency_uses_the_recorded_failure_facts() {
    use tl3_harness_core::finish::{checking_outcomes, summary_outcomes};
    assert!(!checking_outcomes(1, true, false));
    assert!(!checking_outcomes(0, false, true));
    assert!(checking_outcomes(2, false, true));
    assert!(!summary_outcomes(2, false, false));
    assert!(summary_outcomes(2, true, true));
    assert!(!summary_outcomes(1, false, false));
}
