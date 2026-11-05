use super::*;
use io::{Entry, Schedule};
fn failures(count: usize) -> Schedule {
    Schedule {
        entries: vec![Entry::Failed(4); count],
        ..Schedule::default()
    }
}
fn state(s: &io::Run) -> &Value {
    &s.result["state"]
}
/// Q7, state-table.md:270-275: first failure, second failure, per-effect counter.
#[test]
fn mandatory_first_and_second_failure() {
    let one = io::compare(&failures(1));
    assert_eq!(state(&one)["read_failures"], 1);
    assert_eq!(state(&one)["incident"], Value::Null);
    let two = io::compare(&failures(2));
    assert_eq!(state(&two)["read_failures"], 0);
    assert_eq!(state(&two)["read_incidents"], 1);
    assert!(!two.files.is_empty());
    assert!(two.trace.iter().any(|(op, _)| op == "publication.retained"));
}
/// Q7, state-table.md:272-275: probe clears first, never later incidents.
#[test]
fn mandatory_first_probe_and_later_rob_only() {
    for needs_rob in [false, true] {
        let r = io::compare(&Schedule {
            incident: true,
            needs_rob,
            entries: vec![Entry::State],
            ..Schedule::default()
        });
        let incident: Value = serde_json::from_slice(&r.files["incidents/1.read"]).unwrap();
        assert_eq!(incident["open"], needs_rob);
        assert_eq!(r.trace.iter().any(|(op, _)| op == "probe"), !needs_rob);
    }
}
/// #356: two failures inside the same silence period count once.
#[test]
fn mandatory_once_per_silence_period() {
    let r = io::compare(&Schedule {
        frames: vec![(0, 0); 32],
        entries: vec![Entry::Failed(4); 2],
        ..Schedule::default()
    });
    assert_eq!(state(&r)["read_failures"], 1);
    assert_eq!(state(&r)["incident"], Value::Null);
}
/// Q7 / #403: maintenance probes first incidents; postmortems and notices do not escalate.
#[test]
fn mandatory_maintenance_postmortem_notice() {
    let r = io::compare(&Schedule {
        incident: true,
        maintenance: true,
        entries: vec![Entry::State],
        ..Schedule::default()
    });
    assert!(r.trace.iter().any(|(op, _)| op == "probe"));
    for (postmortem, notice) in [(true, false), (false, true)] {
        let r = io::compare(&Schedule {
            postmortem,
            notice,
            failures: 1,
            entries: vec![Entry::Failed(4)],
            ..Schedule::default()
        });
        assert_eq!(state(&r)["read_failures"], 1);
        assert_eq!(state(&r)["incident"], Value::Null);
    }
}
/// #608: a healthy GET cannot clear a refused queue command.
#[test]
fn mandatory_refused_command_hold() {
    let r = io::compare(&Schedule {
        incident: true,
        refused: true,
        entries: vec![Entry::State],
        ..Schedule::default()
    });
    assert!(!r.trace.iter().any(|(op, _)| op == "probe" || op == "look"));
    assert_eq!(state(&r)["due_ms"], Value::Null);
}
/// Q7, state-table.md:309: later per-effect read incidents need Rob.
#[test]
fn mandatory_later_incident() {
    let r = io::compare(&Schedule {
        incidents: 1,
        ..failures(2)
    });
    assert_eq!(state(&r)["read_incidents"], 2);
    let incident: Value = serde_json::from_slice(
        r.files
            .iter()
            .find(|(k, _)| k.starts_with("incidents/"))
            .unwrap()
            .1,
    )
    .unwrap();
    assert_eq!(incident["needs_rob"], true);
}
/// Q7: unrecoverable, quota, open incident and adapter failures retain their order.
#[test]
fn mandatory_error_classes_and_crashes() {
    for code in 0..12 {
        io::compare(&Schedule {
            entries: vec![Entry::Failed(code)],
            ..Schedule::default()
        });
    }
    for entry in [
        Entry::Alive(false),
        Entry::Alive(true),
        Entry::Stall,
        Entry::Probe,
    ] {
        for worker in [false, true] {
            io::compare(&Schedule {
                worker,
                entries: vec![entry],
                ..Schedule::default()
            });
        }
    }
}
/// Q7 / #356: changing reads, independent clocks and crash cuts at each durable boundary.
#[test]
fn differential_schedules() {
    use proptest::{
        prelude::*,
        test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
    };
    let mut runner = TestRunner::new(Config {
        cases: 128,
        max_shrink_iters: 2048,
        rng_seed: RngSeed::Fixed(0x00d1_0760_2026),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/looks_io/proptest-regressions.txt"
        )))),
        ..Config::default()
    });
    let strategy = (
        any::<u16>(),
        prop::collection::vec((any::<u8>(), 0u32..5001), 0..64),
        prop::collection::vec(0u8..3, 0..16),
        prop::collection::vec(0u8..20, 1..10),
    );
    runner
        .run(&strategy, |(bits, frames, writes, steps)| {
            let s = Schedule {
                frames,
                overrides: vec![],
                writes,
                entries: steps
                    .into_iter()
                    .map(|v| match v {
                        0..=11 => Entry::Failed(v),
                        12 => Entry::Alive(false),
                        13 => Entry::Alive(true),
                        14 => Entry::Probe,
                        15 => Entry::Stall,
                        _ => Entry::State,
                    })
                    .collect(),
                failures: (bits % 2) as u8,
                incidents: ((bits >> 1) % 4) as u8,
                incident: bits & 8 != 0,
                needs_rob: bits & 16 != 0,
                refused: bits & 32 != 0,
                maintenance: bits & 64 != 0,
                postmortem: bits & 128 != 0,
                notice: bits & 256 != 0,
                worker: bits & 512 != 0,
                quota: bits & 1024 != 0,
                stopping: bits & 2048 != 0,
                progress: bits & 4096 != 0,
                counted: (bits & 8192 != 0).then_some(1_000_000),
                ..Schedule::default()
            };
            io::compare(&s);
            Ok(())
        })
        .unwrap();
    eprintln!("128 seeded D1 schedules, complete plus before/after every durable write");
}
fn function<'a>(source: &'a str, name: &str, method: bool) -> &'a str {
    let at = source.find(&format!("fn {name}(")).unwrap();
    let start = source[..at].rfind('\n').map_or(0, |p| p + 1);
    let end = source[start..]
        .find(if method { "\n    }\n" } else { "\n}\n" })
        .unwrap()
        + start
        + if method { 7 } else { 3 };
    &source[start..end]
}
/// #760: SHA-256 pins the whole cdacce8f blob, and every compiled body is verbatim.
#[test]
fn source_pins_and_production_copies() {
    let base = include_str!("base-looks.rs.txt");
    assert_eq!(
        format!("{:x}", Sha256::digest(base.as_bytes())),
        "2ad7e468cb0a8d3d22cfe5511b0aa6193cb8236dbca7f8fd7f10a7784b315bee"
    );
    assert_eq!(
        function(include_str!("../../src/looks.rs"), "stall_upstream", false),
        function(include_str!("current.rs"), "stall_upstream", false)
    );
    assert_eq!(
        function(include_str!("../../src/looks.rs"), "incident_view", false),
        function(include_str!("current.rs"), "incident_view", false)
    );
    for name in ["look_kind", "wait_status"] {
        assert_eq!(
            function(include_str!("../../src/looks.rs"), name, false),
            function(include_str!("current.rs"), name, false)
        );
    }
    for (source, copy) in [
        (base, include_str!("reference.rs")),
        (
            include_str!("../../src/looks.rs"),
            include_str!("current.rs"),
        ),
    ] {
        for name in [
            "look_state",
            "look_alive",
            "finish_stall",
            "rearm",
            "look_failed",
            "look_observations",
            "probe_passed",
            "save_allowance",
            "allowance_row",
            "record_progress",
            "progress_allowance",
            "quota_allowance",
            "expire_model_quotas",
        ] {
            assert_eq!(
                function(source, name, true),
                function(copy, name, true),
                "{name}"
            );
        }
        for name in [
            "incident_needs_rob",
            "incident_id",
            "incident_note",
            "failure_class",
            "incident_summary",
            "queue_command_detail",
        ] {
            assert_eq!(
                function(source, name, false),
                function(copy, name, false),
                "{name}"
            );
        }
    }
}

/// Q7, state-table.md:272-275 / #356: the late probe uses a fresh store load.
#[test]
fn mandatory_reprobe_and_probe_outcomes() {
    for probe in [0, 1, 240, 241, 248] {
        let r = io::compare(&Schedule {
            failures: 1,
            entries: vec![Entry::State],
            overrides: vec![("probe".into(), 0, probe)],
            ..Schedule::default()
        });
        assert!(r.trace.iter().any(|(op, _)| op == "look"));
        assert!(r.trace.iter().any(|(op, _)| op == "probe"));
        if probe == 0 {
            assert_eq!(state(&r)["healthy"], true);
            let incident: Value = serde_json::from_slice(
                r.files
                    .iter()
                    .find(|(k, _)| k.starts_with("incidents/"))
                    .unwrap()
                    .1,
            )
            .unwrap();
            assert_eq!(incident["open"], false);
        }
        // Maintenance absorbs a probe error and rearms; own-effect probing calls look_failed.
        for maintenance in [true, false] {
            io::compare(&Schedule {
                incident: true,
                maintenance,
                entries: vec![Entry::State],
                overrides: vec![("probe".into(), 0, probe)],
                ..Schedule::default()
            });
        }
    }
}

/// State-table.md:577 / #356: a worker stops; upstream queue stalls keep distinct classes.
#[test]
fn mandatory_worker_and_queue_stalls() {
    for (worker, progress, expected) in [
        (true, false, "no_progress"),
        (false, false, "queue_unfinished"),
        (false, true, "queue_requeue"),
    ] {
        let r = io::compare(&Schedule {
            worker,
            progress,
            entries: vec![Entry::Alive(false)],
            ..Schedule::default()
        });
        assert_eq!(state(&r)["failure"], expected);
        assert_eq!(r.trace.iter().any(|(op, _)| op == "stop_stalled"), worker);
        assert_eq!(r.trace.iter().any(|(op, _)| op == "observe"), !worker);
    }
}
