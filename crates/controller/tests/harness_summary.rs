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

#[test]
fn rust_generated_harness_schema_has_no_drift() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let committed: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("workers/src/tl3_workers/schemas/harness-wire.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(tl3_controller::check_harness::wire_schema(), committed);
}

/// #823/#760: the old and new shells retain all report/progress/log I/O order,
/// including byte-identical persisted progress at any generated crash point.
#[test]
fn extraction_preserves_recorded_io_schedules_and_crash_state() {
    use proptest::prelude::*;
    use proptest::test_runner::{Config, TestRunner};
    use std::io::{BufRead, BufReader};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // One persistent Python edge runs the real old/new shells. Rust generates
    // schedules, so no deterministic check starts a fresh child per case.
    let mut child = Command::new(root.join("workers/.venv/bin/python"))
        .arg(root.join("workers/tests/harness_io_schedule.py"))
        .env("TL3_BINARY", env!("CARGO_BIN_EXE_tl3"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let output = BufReader::new(output);
    let pipes = std::cell::RefCell::new((input, output));
    let strategy = (
        0u32..100,
        proptest::collection::vec(0u8..16, 0..5),
        proptest::collection::vec(0u32..1_000_000, 0..12),
        proptest::collection::vec(any::<bool>(), 2..5),
        1usize..4,
        0usize..4,
    );
    let coverage = std::cell::RefCell::new(std::collections::BTreeSet::new());
    let boundaries = std::cell::Cell::new(0u64);
    let mut runner = TestRunner::new(Config {
        cases: 8,
        failure_persistence: None,
        ..Config::default()
    });
    let result = runner.run(&strategy, |(initial, reads, times, selector_reads, repeat, waits)| {
        let request =
            serde_json::json!({"initial": initial, "reads": reads, "times": times, "selector_reads": selector_reads, "repeat": repeat, "waits": (0..waits).map(|_| false).chain([true]).collect::<Vec<_>>()});
        let mut pipes = pipes.borrow_mut();
        let (input, output) = &mut *pipes;
        writeln!(input, "{request}").unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        prop_assert!(!line.is_empty(), "persistent extraction oracle exited");
        let result: serde_json::Value = serde_json::from_str(&line).unwrap();
        prop_assert_eq!(result["equal"].as_bool(), Some(true));
        boundaries.set(boundaries.get() + result["crash_boundaries"].as_u64().unwrap());
        for call in result["coverage"].as_array().unwrap() {
            coverage.borrow_mut().insert(call.as_str().unwrap().to_owned());
        }
        Ok(())
    });
    drop(pipes.into_inner().0);
    let status = child.wait().unwrap();
    assert!(status.success(), "extraction oracle failed");
    result.unwrap();
    for required in [
        "selector-exists",
        "proof-is-file",
        "proof-read",
        "stdout-write",
        "stage-write",
        "trusted-source-read",
        "source-file",
        "base-export",
        "assemble-proof",
        "upstream-wait",
        "reattach-read",
        "checkpoint-write",
        "progress-emit",
    ] {
        assert!(
            coverage.borrow().contains(required),
            "unexercised boundary {required}"
        );
    }
    println!(
        "EXTRACTION_IO_AUDIT boundaries={} coverage={:?}",
        boundaries.get(),
        coverage.borrow()
    );
}

#[test]
fn real_cli_preserves_a_python_surrogate_pair_in_identity_comparisons() {
    let output = invoke(
        "identity",
        r#"{"kind":"matches","selectors":[[55296,56320]],"candidate":{"kind":"array","items":[{"kind":"text","codepoints":[55296,56320]}]}}"#,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["result"], true);
}
