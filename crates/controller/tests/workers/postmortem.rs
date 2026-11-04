use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn command(mut command: Command, input: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

fn cli(input: &Value) -> Value {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tl3"));
    cmd.args(["check-reply", "postmortem"]);
    let output = command(cmd, input.to_string().as_bytes());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

pub fn run() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("postmortem/golden.json")).unwrap();
    let mut python = Command::new(super::worker_package().join(".venv/bin/python"));
    python.args(["-c", include_str!("postmortem/legacy.py")]);
    let old = command(python, json!(cases).to_string().as_bytes());
    assert!(
        old.status.success(),
        "{}",
        String::from_utf8_lossy(&old.stderr)
    );
    let old: Vec<Value> = serde_json::from_slice(&old.stdout).unwrap();
    assert_eq!(old.len(), cases.len());
    for (case, expected) in cases.iter().zip(old) {
        assert_eq!(
            expected, case["violations"],
            "legacy golden: {}",
            case["name"]
        );
        assert_eq!(
            cli(case),
            json!({"violations":expected}),
            "Rust/Python parity: {}",
            case["name"]
        );
    }
    println!(
        "test postmortem_golden_python_cli_parity ... ok ({} vectors)",
        cases.len()
    );
    let mut malformed = cases[0].clone();
    malformed["reply"]["postmortem"]["went_wrong"] = json!(["The maker skipped a test."]);
    assert!(!cli(&malformed)["violations"].as_array().unwrap().is_empty());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tl3"));
    cmd.args(["check-reply", "postmortem"]);
    assert!(!command(cmd, b"{broken JSON").status.success());
    println!("test postmortem_cli_schema_and_json_boundary ... ok");
}
