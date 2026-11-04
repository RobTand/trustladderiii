//! #698/#718 extraction conformance: changing reads and crashes are generated
//! once in Rust and replayed through the old and new Python I/O shells.
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use std::io::Write;
use std::process::{Command, Stdio};

fn parity(mode: &str) {
    let schedules = std::cell::RefCell::new(vec![]);
    let mut runner = TestRunner::new(Config {
        cases: 8,
        ..Config::default()
    });
    runner
        .run(&prop::collection::vec(any::<u16>(), 1..12), |schedule| {
            schedules.borrow_mut().push(schedule);
            Ok(())
        })
        .unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let python = root.join("workers/.venv/bin/python");
    let mut child = Command::new(python)
        .arg(root.join("crates/controller/tests/budget_parity/parity.py"))
        .arg(mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&schedules.into_inner()).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
macro_rules! case { ($($name:ident),*) => {$(#[test] fn $name() { parity(stringify!($name)); })*}; }
case!(
    forecast,
    execution,
    cutoff,
    charge,
    history,
    budget_reply,
    allowance,
    compaction,
    io_schedule
);
