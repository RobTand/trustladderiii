#[path = "support/scenario.rs"]
mod scenario;

use std::{
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn child(test: &str, mode: &str) -> Output {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("TL3_WATCHDOG_FIXTURE", mode)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

fn planted(mode: &str) -> bool {
    std::env::var("TL3_WATCHDOG_FIXTURE").as_deref() == Ok(mode)
}

#[test]
fn stalled_scenario_exits_with_last_progress() {
    if planted("stall") {
        scenario::run("planted-worker-wait", Duration::from_millis(100), || {
            scenario::progress("waiting for worker reply");
            loop {
                thread::park();
            }
        });
    }
    let output = child("stalled_scenario_exits_with_last_progress", "stall");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        output.status.code(),
        Some(124),
        "watchdog did not terminate the planted stalled scenario: {diagnostic}"
    );
    assert!(diagnostic.contains("scenario watchdog expired: planted-worker-wait"));
    assert!(diagnostic.contains("last progress: waiting for worker reply"));
    eprintln!("{diagnostic}");
}

#[test]
fn progress_does_not_extend_the_scenario_deadline() {
    if planted("busy") {
        scenario::run("planted-busy-driver", Duration::from_millis(100), || {
            loop {
                scenario::progress("driver still active");
                thread::sleep(Duration::from_millis(1));
            }
        });
    }
    let output = child("progress_does_not_extend_the_scenario_deadline", "busy");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        output.status.code(),
        Some(124),
        "progress must not keep a planted scenario alive: {diagnostic}"
    );
    assert!(diagnostic.contains("scenario watchdog expired: planted-busy-driver"));
    assert!(diagnostic.contains("last progress: driver still active"));
}

#[test]
fn completed_scenarios_disarm_the_watchdog() {
    if planted("complete") {
        let value = scenario::run("completed", Duration::from_millis(100), || 42);
        assert_eq!(value, 42);
        thread::sleep(Duration::from_millis(200));
        return;
    }
    let output = child("completed_scenarios_disarm_the_watchdog", "complete");
    assert!(output.status.success(), "{output:?}");
}
