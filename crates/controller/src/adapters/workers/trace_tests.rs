use super::*;
use std::os::unix::fs::PermissionsExt;

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A stand-in for the release Python. It records its arguments and the log it
/// was handed, and ships the log only while `mlflow-up` exists beside it.
fn fake_python(root: &Path) -> PathBuf {
    let python = root.join("python");
    executable(
        &python,
        "#!/bin/sh\nfor log; do :; done\nprintf '%s\\n' \"$@\" > \"$0.args\"\ncat > \"$0.stdin\"\n\
         [ -e \"$(dirname \"$0\")/mlflow-up\" ] || exit 1\ntouch \"$log.shipped\"\n",
    );
    python
}

/// No launcher, and no fallback Python: tests name the Python in launch.json.
fn none() -> trace::Flusher<'static> {
    trace::Flusher {
        launcher: &[],
        python: Path::new("/nonexistent/python"),
    }
}

const LOGGED: &[u8] = b"{\"kind\":\"start\"}\n";

/// `runs/<run>/attempt-00000000` after its worker ended, with an unshipped span log.
fn ended_attempt(root: &Path) -> (PathBuf, PathBuf) {
    let runs = root.join("runs");
    let attempt = runs.join("72756e").join("attempt-00000000");
    fs::create_dir_all(&attempt).unwrap();
    write(
        &attempt.join("launch.json"),
        &json!({"python": fake_python(root)}),
    )
    .unwrap();
    write(&attempt.join("exit.json"), &json!({"code": -9})).unwrap();
    fs::write(attempt.join("trace.jsonl"), LOGGED).unwrap();
    (runs, attempt)
}

/// Wait for the flusher started for `directory`, if one was.
fn flushed(directory: &Path) -> Option<std::process::ExitStatus> {
    let mut flushes = trace::FLUSHES.lock().unwrap();
    let index = flushes.iter().position(|(path, _)| path == directory)?;
    let (_, mut child) = flushes.remove(index);
    drop(flushes);
    Some(child.wait().unwrap())
}

fn held(runs: &Path) -> PathBuf {
    runs.join(".traces").join("72756e-attempt-00000000")
}

#[test]
fn an_ended_attempt_ships_its_span_log() {
    let root = tempfile::tempdir().unwrap();
    let (runs, attempt) = ended_attempt(root.path());
    fs::write(root.path().join("mlflow-up"), b"").unwrap();
    trace::sweep(
        &runs,
        &none(),
        std::slice::from_ref(&attempt),
        trace::HELD_BYTES,
    )
    .unwrap();
    assert!(
        flushed(&attempt)
            .expect("the ending starts a flusher")
            .success()
    );
    let log = attempt.join("trace.jsonl");
    assert_eq!(
        fs::read_to_string(root.path().join("python.args"))
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            "-I",
            "-m",
            "tl3_workers.tracing",
            "flush",
            log.to_str().unwrap()
        ]
    );
    assert_eq!(fs::read(root.path().join("python.stdin")).unwrap(), LOGGED);
    trace::sweep(
        &runs,
        &none(),
        std::slice::from_ref(&attempt),
        trace::HELD_BYTES,
    )
    .unwrap();
    assert!(flushed(&attempt).is_none(), "a shipped log starts nothing");
}

#[test]
fn a_deliberate_stop_marks_the_attempt_stopped() {
    let root = tempfile::tempdir().unwrap();
    let (runs, attempt) = ended_attempt(root.path());
    fs::remove_file(attempt.join("exit.json")).unwrap();
    let mut worker = Command::new("sleep").arg("600").spawn().unwrap();
    let stat = fs::read_to_string(format!("/proc/{}/stat", worker.id())).unwrap();
    let start = stat.rsplit_once(')').unwrap().1.split_whitespace().nth(19);
    write(
        &attempt.join("process.json"),
        &identity(worker.id(), start.unwrap().parse().unwrap()),
    )
    .unwrap();
    stop_attempt(&runs.join("72756e")).unwrap();
    assert!(
        !worker.wait().unwrap().success(),
        "the stop killed the worker"
    );
    assert!(
        attempt.join("stopped.json").exists(),
        "Q49: a stop is not a crash"
    );
    let other = tempfile::tempdir().unwrap();
    let crashed = ended_attempt(other.path()).1;
    stop_attempt(crashed.parent().unwrap()).unwrap();
    assert!(
        !crashed.join("stopped.json").exists(),
        "a worker that had already died was not stopped"
    );
}

/// Rob's case: MLflow is down when the run retires, and comes back later.
#[test]
fn retirement_holds_an_unshipped_log_until_mlflow_takes_it() {
    let root = tempfile::tempdir().unwrap();
    let (runs, attempt) = ended_attempt(root.path());
    trace::hold(&attempt, &runs).unwrap();
    fs::remove_dir_all(runs.join("72756e")).unwrap();
    let held = held(&runs);
    assert_eq!(fs::read(held.join("trace.jsonl")).unwrap(), LOGGED);
    assert!(held.join("exit.json").exists() && held.join("launch.json").exists());
    trace::sweep(&runs, &none(), &[], trace::HELD_BYTES).unwrap();
    assert!(!flushed(&held).expect("a held log is flushed").success());
    assert!(
        held.join("trace.jsonl").exists(),
        "an unsent log stays held"
    );
    fs::write(root.path().join("mlflow-up"), b"").unwrap();
    trace::sweep(&runs, &none(), &[], trace::HELD_BYTES).unwrap();
    assert!(flushed(&held).expect("the next event retries").success());
    assert_eq!(fs::read(root.path().join("python.stdin")).unwrap(), LOGGED);
    trace::sweep(&runs, &none(), &[], trace::HELD_BYTES).unwrap();
    assert!(flushed(&held).is_none());
    assert!(!held.exists(), "a shipped held log is removed");
}

fn identity(pid: u32, start: u64) -> Value {
    json!({"pid": pid, "start": start,
        "machine": fs::read_to_string("/etc/machine-id").unwrap().trim(),
        "boot": fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap().trim()})
}

/// tl3d restarts while a flusher runs in its own scope: no second flusher;
/// a flusher that died leaves a claim the next event replaces.
#[test]
fn one_flusher_per_attempt_across_restarts() {
    let root = tempfile::tempdir().unwrap();
    let (runs, attempt) = ended_attempt(root.path());
    fs::write(root.path().join("mlflow-up"), b"").unwrap();
    let claim = attempt.join("trace.jsonl.flushing");
    let stat = fs::read_to_string("/proc/self/stat").unwrap();
    let start = stat.rsplit_once(')').unwrap().1.split_whitespace().nth(19);
    write(
        &claim,
        &identity(std::process::id(), start.unwrap().parse().unwrap()),
    )
    .unwrap();
    trace::sweep(
        &runs,
        &none(),
        std::slice::from_ref(&attempt),
        trace::HELD_BYTES,
    )
    .unwrap();
    assert!(
        flushed(&attempt).is_none(),
        "a live flusher keeps the attempt"
    );
    let mut gone = Command::new("true").spawn().unwrap();
    gone.wait().unwrap();
    write(&claim, &identity(gone.id(), 1)).unwrap();
    trace::sweep(
        &runs,
        &none(),
        std::slice::from_ref(&attempt),
        trace::HELD_BYTES,
    )
    .unwrap();
    assert!(
        flushed(&attempt)
            .expect("a dead flusher is replaced")
            .success()
    );
    assert!(
        !claim.exists(),
        "the stale claim is cleared for the new flusher"
    );
}

#[test]
fn flushers_start_in_the_launcher_scope() {
    let root = tempfile::tempdir().unwrap();
    let (runs, attempt) = ended_attempt(root.path());
    fs::write(root.path().join("mlflow-up"), b"").unwrap();
    let launcher = root.path().join("launcher");
    executable(
        &launcher,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\nshift\nexec \"$@\"\n",
    );
    let launcher = vec![
        launcher.to_str().unwrap().to_owned(),
        "--scope-fixture".into(),
    ];
    let python = PathBuf::new();
    trace::sweep(
        &runs,
        &trace::Flusher {
            launcher: &launcher,
            python: &python,
        },
        std::slice::from_ref(&attempt),
        trace::HELD_BYTES,
    )
    .unwrap();
    assert!(flushed(&attempt).unwrap().success());
    let args = fs::read_to_string(root.path().join("launcher.args")).unwrap();
    let args: Vec<_> = args.lines().collect();
    assert_eq!(args[0], "--scope-fixture");
    assert_eq!(args[1], root.path().join("python").to_str().unwrap());
    assert_eq!(args[2..6], ["-I", "-m", "tl3_workers.tracing", "flush"]);
}

#[test]
fn held_logs_beyond_the_cap_drop_oldest_first() {
    let root = tempfile::tempdir().unwrap();
    let runs = root.path().join("runs");
    let base = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    for (index, name) in [
        "b-attempt-00000000",
        "a-attempt-00000000",
        "c-attempt-00000000",
    ]
    .iter()
    .enumerate()
    {
        let directory = runs.join(".traces").join(name);
        fs::create_dir_all(&directory).unwrap();
        // Retirement order, not name order: b first, then a, then c.
        fs::write(directory.join("trace.jsonl"), [b'x'; 100]).unwrap();
        File::options()
            .write(true)
            .open(directory.join("trace.jsonl"))
            .unwrap()
            .set_modified(base + std::time::Duration::from_secs(index as u64))
            .unwrap();
    }
    let evicted = trace::sweep(&runs, &none(), &[], 250).unwrap();
    assert_eq!(evicted, 1);
    assert!(!runs.join(".traces/b-attempt-00000000").exists());
    assert!(runs.join(".traces/a-attempt-00000000").exists());
    assert!(runs.join(".traces/c-attempt-00000000").exists());
}

/// A crash between moving files and publishing the held directory leaves them
/// staged in the attempt; retirement's retry finishes the hold.
#[test]
fn an_interrupted_hold_finishes_on_retry() {
    let root = tempfile::tempdir().unwrap();
    let (runs, attempt) = ended_attempt(root.path());
    let staging = attempt.join(".held");
    fs::create_dir(&staging).unwrap();
    fs::rename(attempt.join("trace.jsonl"), staging.join("trace.jsonl")).unwrap();
    trace::hold(&attempt, &runs).unwrap();
    let held = held(&runs);
    assert_eq!(fs::read(held.join("trace.jsonl")).unwrap(), LOGGED);
    assert!(held.join("exit.json").exists() && held.join("launch.json").exists());
    assert!(!staging.exists());
}

/// #448 re-check: the outage-then-recovery trigger, end to end through the
/// worker source's inotify watch on `.traces`.
#[test]
fn a_wake_write_sweeps_held_logs_through_the_source() {
    use crate::git_tests::support::Database;
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::start();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create("trace_wake");
    config.workers.runs = root.path().join("runs");
    let supervisor = Supervisor::new(&config, &root.path().join("records")).unwrap();
    let held = held(&config.workers.runs);
    fs::create_dir_all(&held).unwrap();
    write(
        &held.join("launch.json"),
        &json!({"python": fake_python(root.path())}),
    )
    .unwrap();
    fs::write(held.join("trace.jsonl"), LOGGED).unwrap();
    let mut source = supervisor.source();
    let mut store = Store::open(&config.store).unwrap();
    let (sender, _receiver) = crate::sync::mpsc::channel();
    // tl3d opens while MLflow is down: the held log's flush fails.
    source.poll_once(&mut store, &sender).unwrap();
    assert!(!flushed(&held).expect("opening flushes held logs").success());
    source.poll_once(&mut store, &sender).unwrap();
    assert!(flushed(&held).is_none(), "no event, no flush");
    // MLflow answers a shipper, which writes the wake file.
    fs::write(root.path().join("mlflow-up"), b"").unwrap();
    let wake = config.workers.runs.join(".traces/.wake");
    drop(
        File::options()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&wake)
            .unwrap(),
    );
    source.poll_once(&mut store, &sender).unwrap();
    assert!(flushed(&held).expect("the wake starts a flusher").success());
    drop(File::options().write(true).open(&wake).unwrap());
    source.poll_once(&mut store, &sender).unwrap();
    assert!(
        !held.exists(),
        "the next event removes the shipped held log"
    );
    drop(source);
    drop(supervisor);
    db.stop();
}
