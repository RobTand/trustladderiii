use std::io::Write;
use std::process::{Command, Stdio};

fn invoke(kind: &str, input: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tl3"))
        .args(["check-harness", kind])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn real_cli_bounds_a_summary_and_returns_one_complete_document() {
    let output = invoke(
        "summary",
        r#"{"kind":"bounded","summary_json":"{\"status\":\"completed\",\"tests\":[],\"totals\":{},\"note\":\"\"}","limit":16384}"#,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout.iter().filter(|byte| **byte == b'\n').count(),
        1
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result["result"]["summary_json"].is_string(), "{result}");
    assert_eq!(result["result"]["report"]["diagnostic"], false);
}

#[test]
fn real_cli_uses_the_shared_ordered_selector_identity() {
    let output = invoke("identity", r#"{"kind":"identity","selectors":[]}"#);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["result"]["schema"], "tl3.selectors.v1");
    assert_eq!(
        result["result"]["sha256"],
        "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"
    );
}

#[test]
fn real_cli_rejects_duplicate_keys_nonfinite_values_and_unknown_fields() {
    for input in [
        r#"{"kind":"identity","kind":"identity","selectors":[]}"#,
        r#"{"kind":"identity","selectors":[],"extra":NaN}"#,
        r#"{"kind":"identity","selectors":[],"extra":true}"#,
    ] {
        let output = invoke("identity", input);
        assert!(!output.status.success(), "accepted {input}");
        assert!(output.stdout.is_empty());
    }
}
