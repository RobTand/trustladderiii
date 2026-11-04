use serde_json::{Value, json};
use std::process::{Command, Stdio};
use tl3_controller::reviewer::validate;

pub fn run() {
    golden_cases();
    println!("test reviewer_golden_cases ... ok");
    python_parity();
    println!("test reviewer_python_parity ... ok");
    cli_contract();
    println!("test reviewer_cli_contract ... ok");
}

fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("reviewer/golden.json")).unwrap()
}

fn golden_cases() {
    let cases = cases();
    assert!(cases.len() >= 50);
    let mut mismatches = Vec::new();
    for case in cases {
        let result = validate(&case["inputs"], &case["reply"]);
        if case["schema_invalid"] == true {
            assert!(result.is_err(), "{case}");
        } else {
            let actual: Vec<_> = result.err().map(|e| e.to_string()).into_iter().collect();
            if json!(actual) != case["violations"] {
                mismatches.push(
                    json!({"case":case["name"],"actual":actual,"expected":case["violations"]}),
                );
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

fn command(mut command: Command, input: &Value) -> std::process::Output {
    use std::io::Write;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result
}

fn python_parity() {
    let cases: Vec<_> = cases()
        .into_iter()
        .filter(|c| c["schema_invalid"] != true)
        .collect();
    let mut python = Command::new(super::worker_package().join(".venv/bin/python"));
    python.args(["-c", include_str!("reviewer/legacy.py")]);
    let results: Vec<Value> =
        serde_json::from_slice(&command(python, &json!(cases)).stdout).unwrap();
    assert_eq!(results.len(), cases.len());
    for (case, expected) in cases.iter().zip(results) {
        assert_eq!(
            case["violations"], expected,
            "Python oracle: {}",
            case["name"]
        );
        let actual: Vec<_> = validate(&case["inputs"], &case["reply"])
            .err()
            .map(|e| e.to_string())
            .into_iter()
            .collect();
        assert_eq!(json!(actual), expected, "Rust parity: {}", case["name"]);
    }
}

fn cli_contract() {
    let case = &cases()[2];
    let mut cli = Command::new(env!("CARGO_BIN_EXE_tl3"));
    cli.args(["check-reply", "reviewer"]);
    let result = command(
        cli,
        &json!({"inputs":case["inputs"], "reply":case["reply"]}),
    );
    assert!(result.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        json!({"violations":case["violations"]})
    );
}
