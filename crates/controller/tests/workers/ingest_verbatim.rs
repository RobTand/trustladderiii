use std::io::Write;
use std::process::{Command, Stdio};

/// #840: real class-D issues retain their supplied tests at both downstream inputs.
pub fn run() {
    let mut child = Command::new(super::worker_package().join(".venv/bin/python"))
        .args(["-c", include_str!("ingest_verbatim/worker.py")])
        .env("TL3_BINARY", env!("CARGO_BIN_EXE_tl3"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(include_bytes!("ingest_verbatim/golden.json"))
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    println!("test ingest_verbatim_real_issue_tests_reach_planner_and_maker ... ok");
}
