//! #698/#718 extraction conformance: changing reads and crashes are generated
//! once in Rust and replayed through the old and new Python I/O shells.
use proptest::prelude::*;
use proptest::test_runner::{Config, TestError, TestRunner};
use std::io::Write;
use std::process::{Command, Stdio};

struct Python {
    child: std::process::Child,
    input: std::process::ChildStdin,
    output: std::io::BufReader<std::process::ChildStdout>,
}
impl Python {
    fn new(mode: &str) -> Self {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut child = Command::new(root.join("workers/.venv/bin/python"))
            .arg(root.join("crates/controller/tests/budget_parity/parity.py"))
            .arg(mode)
            .arg("--server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = std::io::BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            input,
            output,
        }
    }
    fn check(&mut self, schedule: &[u16]) -> serde_json::Value {
        use std::io::BufRead;
        serde_json::to_writer(&mut self.input, schedule).unwrap();
        self.input.write_all(b"\n").unwrap();
        self.input.flush().unwrap();
        let mut line = String::new();
        assert!(
            self.output.read_line(&mut line).unwrap() > 0,
            "Python oracle exited"
        );
        serde_json::from_str(&line).expect("invalid oracle response")
    }
}
impl Drop for Python {
    fn drop(&mut self) {
        let _ = self.child.kill();
        self.child.wait().unwrap();
    }
}
fn run_property(
    mode: &str,
    cases: u32,
    regressions: &std::path::Path,
    seed: u64,
) -> Result<(), TestError<Vec<u16>>> {
    use proptest::test_runner::{FileFailurePersistence, RngSeed};
    let python = std::cell::RefCell::new(Python::new(mode));
    let path = Box::leak(regressions.to_string_lossy().into_owned().into_boxed_str());
    let mut runner = TestRunner::new(Config {
        cases,
        max_shrink_iters: 8192,
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(path))),
        ..Config::default()
    });
    runner.run(&prop::collection::vec(any::<u16>(), 1..12), |schedule| {
        let response = python.borrow_mut().check(&schedule);
        prop_assert!(
            response["ok"] == true,
            "{}",
            response["error"].as_str().unwrap_or("missing oracle error")
        );
        Ok(())
    })
}
fn parity(mode: &str) {
    let directory = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = directory.join(format!("t780-{mode}-regressions.txt"));
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/budget_parity/{mode}-regressions.txt"));
    std::fs::copy(source, &path).unwrap();
    let seed = mode.bytes().fold(780u64, |seed, byte| {
        seed.wrapping_mul(31).wrapping_add(u64::from(byte))
    });
    run_property(mode, 8, &path, seed).unwrap();
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

/// #780: removing any durable session.save stage must break real-I/O conformance.
#[test]
fn io_checkpoint_mutants() {
    let response = Python::new("io_checkpoint_mutants").check(&[0, 1, 2]);
    assert!(response["ok"] == true, "{}", response["error"]);
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
    diagnostics
);
