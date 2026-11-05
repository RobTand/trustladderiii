use super::*;
use io::{Entry, Schedule};
fn schedule() -> Schedule {
    Schedule {
        d2: true,
        entries: vec![Entry::State],
        ..Schedule::default()
    }
}
fn has(r: &io::Run, op: &str) -> bool {
    r.trace.iter().any(|(name, _)| name == op)
}
fn persisted(r: &io::Run) -> Value {
    serde_json::from_slice(&r.files["allowances/effect-1"]).unwrap()
}
/// Q53, state-table.md:277,792 / #759: wait only until reset or the one ladder-out note.
#[test]
fn mandatory_d2_quota_wait_and_ladder_bound() {
    for (since, noticed, note) in [
        (None, false, false),
        (Some(999_000), false, false),
        (Some(0), false, true),
        (Some(0), true, false),
    ] {
        let r = io::compare(&Schedule {
            announced: Some(4_000_000),
            since,
            noticed,
            frames: vec![(0, 1_000_000)],
            ..schedule()
        });
        assert!(has(&r, "store.load") && has(&r, "quota_resume"));
        assert_eq!(has(&r, "ladder_note"), note);
        assert!(!has(&r, "look"));
        assert!(persisted(&r)["due_ms"].as_u64().unwrap() <= 4_000_000);
        if note {
            assert_eq!(persisted(&r)["ladder_out_noticed"], true);
            let repeated = io::compare(&Schedule {
                announced: Some(4_000_000),
                since,
                noticed,
                entries: vec![Entry::State, Entry::State],
                frames: std::iter::once((0, 1_000_000))
                    .chain(std::iter::repeat_n((0, 0), 63))
                    .collect(),
                ..schedule()
            });
            assert_eq!(
                repeated
                    .trace
                    .iter()
                    .filter(|(op, _)| op == "ladder_note")
                    .count(),
                1
            );
        }
    }
}
/// Preserved existing behaviour (no rule): quota recovery reads confirmed effects before resuming.
#[test]
fn mandatory_d2_quota_resume() {
    for effect in [0, 1] {
        for resume in [0, 1, 2, 3, 244] {
            let r = io::compare(&Schedule {
                quota: true,
                overrides: vec![
                    ("store.effect".into(), 0, effect),
                    ("resume_quota".into(), 0, resume),
                ],
                ..schedule()
            });
            assert_eq!(has(&r, "resume_quota"), effect == 1);
            assert!(!has(&r, "look"));
            if effect == 1 && resume >= 2 && resume < 240 {
                assert!(has(&r, "queue_observation"));
            }
        }
    }
}
/// Extraction fidelity: every look kind, pending read and repeated-404 recovery is reachable.
#[test]
fn mandatory_d2_each_look_and_not_found() {
    for look in 0..12 {
        let r = io::compare(&Schedule {
            worker: look == 7,
            overrides: vec![("look".into(), 0, look)],
            ..schedule()
        });
        assert!(has(&r, "look"));
        match look {
            5 | 10 | 11 => assert!(has(&r, "complete")),
            7 => assert!(has(&r, "wait_record")),
            8 => assert!(has(&r, "watch_wait")),
            9 => assert!(has(&r, "record_progress") || has(&r, "store.loaded")),
            _ => {}
        }
    }
    let r = io::compare(&Schedule {
        not_found: true,
        overrides: vec![("look".into(), 0, 246)],
        ..schedule()
    });
    assert!(has(&r, "complete"));
    for (held, checks, effect, retry) in [
        (true, false, 0, true),
        (true, false, 2, false),
        (false, true, 0, false),
        (false, false, 0, false),
    ] {
        let r = io::compare(&Schedule {
            held,
            checks,
            overrides: vec![("look".into(), 0, 3), ("store.effect".into(), 0, effect)],
            ..schedule()
        });
        assert_eq!(has(&r, "complete"), retry);
        assert_eq!(has(&r, "remove_hold"), checks);
    }
}
/// #373: a stalled pending effect retains failures; timely live reads clear them.
#[test]
fn mandatory_d2_alive_allowance_and_effect_read() {
    for recent in [false, true] {
        for effect in [0, 1] {
            let r = io::compare(&Schedule {
                failures: 1,
                progress: true,
                recent_progress: recent,
                frames: vec![(0, 0); 64],
                overrides: vec![
                    ("look".into(), 0, 6),
                    ("store.effect".into(), 0, effect),
                    ("store.effect".into(), 1, effect),
                ],
                ..schedule()
            });
            assert_eq!(has(&r, "store.effect"), !recent);
            if recent || effect == 1 {
                assert_eq!(r.result["state"]["read_failures"], 0);
            }
        }
    }
}
/// Q43, state-table.md:800: only the outside job record ends a declared wait.
#[test]
fn mandatory_d2_declared_waits() {
    for ended in [false, true] {
        for record in [0, 1, 244] {
            let r = io::compare(&Schedule {
                worker: true,
                waiting: true,
                wait_ended: ended,
                overrides: vec![("look".into(), 0, 7), ("wait_record".into(), 0, record)],
                ..schedule()
            });
            assert!(has(&r, "wait_record"));
            assert_eq!(has(&r, "watch_wait"), record < 240);
        }
    }
    let r = io::compare(&Schedule {
        waiting: true,
        overrides: vec![("look".into(), 0, 1)],
        ..schedule()
    });
    assert!(has(&r, "watch_wait"));
    let r = io::compare(&Schedule {
        stopping: true,
        ..schedule()
    });
    assert!(has(&r, "stop_stalled"));
    assert!(!has(&r, "look"));
}
/// Preserved existing behaviour (no rule): progress retires waits; quota replay never restarts the wait.
#[test]
fn mandatory_d2_progress_and_quota_allowances() {
    let r = io::compare(&Schedule {
        waiting: true,
        entries: vec![Entry::Progress(0), Entry::Progress(0)],
        ..schedule()
    });
    assert_eq!(
        r.trace.iter().filter(|(op, _)| op == "retire_wait").count(),
        1
    );
    assert_eq!(persisted(&r)["unserved_since_ms"], Value::Null);
    let r = io::compare(&Schedule {
        entries: vec![
            Entry::Quota(1),
            Entry::Quota(1),
            Entry::Quota(3),
            Entry::Quota(2),
        ],
        ..schedule()
    });
    assert_eq!(r.trace.iter().filter(|(op, _)| op == "clock").count(), 6);
    assert_eq!(persisted(&r)["last_quota_model"], Value::Null);
    assert!(r.files.contains_key("model_quotas/model-a"));
}
/// Preserved existing behaviour (no rule): setup gaps bypass completion deduplication, and changed settled facts are observed once.
#[test]
fn mandatory_d2_settled_completion_shapes() {
    for look in [5, 9, 10, 11, 12, 13] {
        for recorded in [0, 1] {
            let r = io::compare(&Schedule {
                entries: vec![Entry::State, Entry::State],
                frames: vec![(0, 0); 64],
                overrides: vec![
                    ("look".into(), 0, look),
                    ("look".into(), 1, look),
                    ("store.effect".into(), 0, 1),
                    ("store.effect".into(), 1, 1),
                    ("queue.recorded".into(), 0, recorded),
                    ("queue.recorded".into(), 1, recorded),
                ],
                ..schedule()
            });
            assert!(!has(&r, "complete"));
            let observations = r
                .trace
                .iter()
                .filter(|(op, _)| op == "queue_observation")
                .count();
            assert_eq!(
                observations,
                match look {
                    5 | 9 => 0,
                    12 if recorded == 0 => 2,
                    _ => 1,
                }
            );
            assert_eq!(has(&r, "queue.recorded"), look == 12);
        }
    }
}
/// D2 I/O fidelity: independent read responses and crash cuts at every recorded write.
#[test]
fn differential_d2_schedules() {
    use proptest::{
        prelude::*,
        test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
    };
    let mut runner = TestRunner::new(Config {
        cases: 128,
        max_shrink_iters: 2048,
        rng_seed: RngSeed::Fixed(0x00d2_0760_2026),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/looks_io/proptest-regressions.txt"
        )))),
        ..Config::default()
    });
    runner
        .run(
            &(
                any::<u16>(),
                prop::collection::vec((any::<u8>(), 0u32..2_000_001), 0..80),
                prop::collection::vec(0u8..3, 0..20),
                prop::collection::vec(0u8..8, 1..8),
            ),
            |(bits, frames, writes, steps)| {
                io::compare(&Schedule {
                    d2: true,
                    frames,
                    writes,
                    entries: steps
                        .into_iter()
                        .map(|v| match v {
                            0..=3 => Entry::State,
                            4 => Entry::Progress(0),
                            5 => Entry::Progress(1),
                            6 => Entry::Quota(1),
                            _ => Entry::Quota(3),
                        })
                        .collect(),
                    quota: bits & 1 != 0,
                    waiting: bits & 2 != 0,
                    wait_ended: bits & 4 != 0,
                    worker: bits & 8 != 0,
                    held: bits & 16 != 0,
                    checks: bits & 32 != 0,
                    not_found: bits & 64 != 0,
                    noticed: bits & 128 != 0,
                    since: (bits & 256 != 0).then_some(0),
                    announced: (bits & 512 != 0).then_some(4_000_000),
                    recent_progress: bits & 1024 != 0,
                    stopping: bits & 2048 != 0,
                    ..Schedule::default()
                });
                Ok(())
            },
        )
        .unwrap();
    eprintln!("128 seeded D2 schedules plus every before/after durable crash cut");
}
