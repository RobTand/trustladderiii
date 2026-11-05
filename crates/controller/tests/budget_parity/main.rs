//! #698/#718 extraction conformance: changing reads and crashes are generated
//! once in Rust and replayed through the old and new Python I/O shells.
use proptest::prelude::*;
use proptest::test_runner::{Config, TestError, TestRunner};
use std::io::Write;
use std::process::{Command, Stdio};

fn run_property(
    mode: &str,
    cases: u32,
    _regressions: &std::path::Path,
    _seed: u64,
) -> Result<(), TestError<Vec<u16>>> {
    let schedules = std::cell::RefCell::new(vec![]);
    let mut runner = TestRunner::new(Config {
        cases,
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
    if output.status.success() {
        Ok(())
    } else {
        Err(TestError::Fail(
            String::from_utf8_lossy(&output.stderr).to_string().into(),
            vec![],
        ))
    }
}
fn parity(mode: &str) {
    let path = std::env::temp_dir().join(format!("t780-{mode}-regressions.txt"));
    run_property(mode, 8, &path, 780).unwrap();
}

/// #780: Python assertion failures must shrink and persist for deterministic replay.
#[test]
fn python_failures_shrink_and_persist() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("regressions.txt");
    let first = run_property("shrinking_probe", 8, &path, 7803);
    assert!(
        matches!(first, Err(TestError::Fail(_, ref schedule)) if schedule == &[4]),
        "{first:?}"
    );
    assert!(path.is_file(), "failing seed was not persisted");
    let replay = run_property("shrinking_probe", 0, &path, 98765);
    assert!(
        matches!(replay, Err(TestError::Fail(_, ref schedule)) if schedule == &[4]),
        "{replay:?}"
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
    io_schedule,
    io_checkpoint_mutants,
    diagnostics
);
