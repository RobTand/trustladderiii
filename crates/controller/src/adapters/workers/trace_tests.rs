use super::*;
use std::os::unix::fs::PermissionsExt;

/// An attempt whose worker ended, with a stand-in for the release Python that
/// records its arguments and the log it was handed.
fn ended_attempt(root: &Path) -> PathBuf {
    let attempt = root.join("attempt-00000000");
    fs::create_dir(&attempt).unwrap();
    let python = root.join("python");
    fs::write(
        &python,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\ncat > \"$0.stdin\"\n",
    )
    .unwrap();
    fs::set_permissions(&python, fs::Permissions::from_mode(0o755)).unwrap();
    write(&attempt.join("launch.json"), &json!({"python": python})).unwrap();
    write(&attempt.join("exit.json"), &json!({"code": -9})).unwrap();
    fs::write(attempt.join("trace.jsonl"), b"{\"kind\":\"start\"}\n").unwrap();
    attempt
}

fn started(attempt: &Path) -> Option<Child> {
    let mut flushes = trace::FLUSHES.lock().unwrap();
    let index = flushes.iter().position(|(path, _)| path == attempt)?;
    Some(flushes.remove(index).1)
}

fn delivery() -> Delivery {
    serde_json::from_value(json!({
        "key":"trace-flush", "epoch":1,
        "output":{"Effect":{"node":0,"kind":{"StartRun":"Ingest"},"seq":1,"head":null}},
        "generation":1, "head_sha":null,
        "repository":{"owner":"RobTand","name":"tl3-sandbox","installation":1},
        "issue":42, "pull_request":null, "paths":[], "attempt":1, "round":0, "checks":null,
    }))
    .unwrap()
}

fn assert_flushed(root: &Path, attempt: &Path, child: Option<Child>) {
    let mut child = child.expect("an ended attempt with an unshipped span log starts the flusher");
    assert!(child.wait().unwrap().success());
    let args = fs::read_to_string(root.join("python.args")).unwrap();
    let log = attempt.join("trace.jsonl");
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        [
            "-I",
            "-m",
            "tl3_workers.tracing",
            "flush",
            log.to_str().unwrap()
        ]
    );
    assert_eq!(
        fs::read(root.join("python.stdin")).unwrap(),
        b"{\"kind\":\"start\"}\n"
    );
}

#[test]
fn an_ended_attempt_ships_its_span_log() {
    let root = tempfile::tempdir().unwrap();
    let attempt = ended_attempt(root.path());
    assert!(matches!(
        inspect(&attempt, &delivery()).unwrap(),
        Reconciliation::Found(_)
    ));
    assert_flushed(root.path(), &attempt, started(&attempt));
}

#[test]
fn a_stopped_attempt_ships_its_log_even_after_retirement_deletes_it() {
    let root = tempfile::tempdir().unwrap();
    let attempt = ended_attempt(root.path());
    fs::remove_file(attempt.join("exit.json")).unwrap();
    stop_attempt(root.path()).unwrap();
    let child = started(&attempt);
    // Retirement removes the run root; the flusher already holds the log open.
    fs::remove_dir_all(&attempt).unwrap();
    assert_flushed(root.path(), &attempt, child);
}

#[test]
fn a_shipped_or_absent_log_starts_nothing() {
    let root = tempfile::tempdir().unwrap();
    let attempt = ended_attempt(root.path());
    fs::write(attempt.join("trace.jsonl.shipped"), b"").unwrap();
    assert!(trace::start_flush(&attempt).unwrap().is_none());
    fs::remove_file(attempt.join("trace.jsonl.shipped")).unwrap();
    fs::remove_file(attempt.join("trace.jsonl")).unwrap();
    assert!(trace::start_flush(&attempt).unwrap().is_none());
    fs::remove_file(attempt.join("launch.json")).unwrap();
    fs::write(attempt.join("trace.jsonl"), b"").unwrap();
    assert!(
        trace::start_flush(&attempt).unwrap().is_none(),
        "without a recorded Python there is no flusher to start"
    );
}
