//! A check backend's report for scenario executors that stand in for the Flux checks
//! adapter. Results follow the harness report shape (one result per planned check, with
//! the planned proof identity), failed tests come from those results, and changed paths
//! come from the head's `base...head` diff, as production stages them.
use serde_json::{Value, json};
use std::{path::Path, process::Command};
use tl3_controller::{
    executor::{Delivery, Observation},
    findings::FailedTest,
};

/// The failure the scenarios' greeting fixture reports.
pub const GREETING_FAILURE: &str = "assert 'goodbye' == 'hello'";

/// `fail` fails the first check's tests with [`GREETING_FAILURE`]. `recorded` replaces
/// the reported results with a recorded run's.
pub fn report(
    delivery: &Delivery,
    action_key: String,
    fail: bool,
    recorded: Option<Vec<Value>>,
) -> Observation {
    let run = delivery
        .checks
        .as_ref()
        .expect("a checks delivery carries its run");
    let head = delivery
        .head_sha
        .clone()
        .expect("a checks delivery carries its head");
    let mut results: Vec<Value> = run.plan.iter().map(planned_result).collect();
    if fail && let Some(first) = results.first_mut() {
        first["verdict"] = json!("fail");
        for test in first["tests"].as_array_mut().unwrap() {
            test["head"] = json!("assertion");
            test["passed"] = json!(false);
            test["message"] = json!(GREETING_FAILURE);
        }
    }
    if let Some(recorded) = recorded {
        results = recorded;
    }
    let failed_tests = results
        .iter()
        .flat_map(|result| result["tests"].as_array().unwrap())
        .filter(|test| test["passed"] == false)
        .map(|test| FailedTest {
            test: test["test"].as_str().unwrap().into(),
            message: test["message"].as_str().unwrap_or_default().into(),
        })
        .collect();
    Observation::Checks {
        node: delivery.node(),
        generation: delivery.generation,
        action_key,
        base_sha: run.base_sha.as_str().to_owned(),
        changed_paths: changed_paths(&run.repository, run.base_sha.as_str(), &head),
        sha: head,
        failed_tests,
        report: true,
        pass: results.iter().all(|result| result["verdict"] == "pass"),
        results,
    }
}

fn planned_result(check: &tl3_controller::checks::Check) -> Value {
    let check = serde_json::to_value(check).unwrap();
    let base = if matches!(
        check["kind"].as_str(),
        Some("demonstrating" | "reproducing")
    ) {
        "assertion"
    } else {
        "passed"
    };
    let tests: Vec<Value> = check["tests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|test| {
            json!({"test": test, "base": base, "head": "passed", "pre_existing": false, "passed": true})
        })
        .collect();
    let mut result = json!({"name": check["name"], "kind": check["kind"],
        "custody": "in-process", "verdict": "pass", "tests": tests});
    if let Some(declaration) = check
        .get("proof")
        .filter(|proof| proof["subjects"].as_array().is_some_and(Vec::is_empty))
    {
        let forced = !check["forcing"].is_null() || !check["declared_symptom"].is_null();
        let observers: serde_json::Map<String, Value> = check["tests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|test| (test.as_str().unwrap().to_owned(), json!("f".repeat(64))))
            .collect();
        result["proof"] = json!({"rule": "subject-observer-v1", "declaration": declaration,
            "mode": if forced { "pytest-outcome" } else { "head-observer" },
            "subjects": {"value.py": {"base": "a".repeat(64), "head": "b".repeat(64)}},
            "source_base_tree_sha256": "c".repeat(64), "base_tree_sha256": "d".repeat(64),
            "head_tree_sha256": "e".repeat(64), "observer_sha256": observers, "support_sha256": {}});
    }
    result
}

fn changed_paths(repository: &Path, base: &str, head: &str) -> Vec<String> {
    let output = Command::new("git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "diff",
            "--name-only",
            "--no-renames",
        ])
        .arg(format!("{base}...{head}"))
        .current_dir(repository)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}
