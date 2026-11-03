#[path = "support/scenario.rs"]
mod scenario;

use std::{
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn child(test: &str, mode: &str) -> (Output, Duration) {
    let started = Instant::now();
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
    (child.wait_with_output().unwrap(), started.elapsed())
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
    let (output, elapsed) = child("stalled_scenario_exits_with_last_progress", "stall");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        output.status.code(),
        Some(124),
        "watchdog did not terminate the planted stalled scenario: {diagnostic}"
    );
    assert!(diagnostic.contains("scenario watchdog expired: planted-worker-wait"));
    assert!(diagnostic.contains("last progress: waiting for worker reply"));
    assert!(elapsed >= Duration::from_millis(100), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    eprintln!("{diagnostic}");
}

#[test]
fn steady_progress_outlives_the_inactivity_limit() {
    if planted("steady") {
        scenario::run("planted-steady-driver", Duration::from_millis(200), || {
            for _ in 0..20 {
                scenario::progress("applied observation");
                thread::sleep(Duration::from_millis(25));
            }
        });
        return;
    }
    let (output, elapsed) = child("steady_progress_outlives_the_inactivity_limit", "steady");
    assert!(
        output.status.success(),
        "steady progress must keep the scenario alive: {output:?}"
    );
    assert!(elapsed >= Duration::from_millis(500), "{elapsed:?}");
}

#[test]
fn perpetual_progress_expires_at_the_absolute_cap() {
    if planted("busy") {
        scenario::run_with_limits(
            "planted-busy-driver",
            Duration::from_millis(200),
            Duration::from_millis(700),
            || loop {
                scenario::progress("driver still active");
                thread::sleep(Duration::from_millis(25));
            },
        );
    }
    let (output, elapsed) = child("perpetual_progress_expires_at_the_absolute_cap", "busy");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(124), "{diagnostic}");
    assert!(
        elapsed >= Duration::from_millis(700),
        "absolute cap expired early: {elapsed:?}; {diagnostic}"
    );
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
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
    let (output, _) = child("completed_scenarios_disarm_the_watchdog", "complete");
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn stalled_fixture_wait_exits_with_last_progress() {
    if planted("wait-stall") {
        scenario::wait(
            "planted-fixture-wait",
            Duration::from_millis(200),
            Duration::from_secs(2),
            || -> Option<()> {
                scenario::progress("planner reply applied");
                loop {
                    thread::park();
                }
            },
        );
    }
    let (output, elapsed) = child(
        "stalled_fixture_wait_exits_with_last_progress",
        "wait-stall",
    );
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(124), "{diagnostic}");
    assert!(
        diagnostic.contains("last progress: planner reply applied"),
        "{diagnostic}"
    );
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    eprintln!("{diagnostic}");
}

#[test]
fn slow_fixture_wait_passes_while_observations_advance() {
    if planted("wait-steady") {
        let mut observations = 0;
        let value = scenario::wait(
            "planted-slow-fixture",
            Duration::from_millis(200),
            Duration::from_secs(2),
            || {
                thread::sleep(Duration::from_millis(25));
                observations += 1;
                scenario::progress(format!("applied observation {observations}"));
                (observations == 20).then_some(observations)
            },
        );
        assert_eq!(value, 20);
        return;
    }
    let (output, elapsed) = child(
        "slow_fixture_wait_passes_while_observations_advance",
        "wait-steady",
    );
    assert!(
        output.status.success(),
        "progressing fixture failed: {output:?}"
    );
    assert!(elapsed >= Duration::from_millis(500), "{elapsed:?}");
}
