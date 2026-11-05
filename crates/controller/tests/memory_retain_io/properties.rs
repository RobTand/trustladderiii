use super::*;
use io::{Entry, Schedule};
use proptest::{
    prelude::*,
    test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
};
include!("names.rs");
fn function<'a>(source: &'a str, name: &str) -> &'a str {
    let re = Regex::new(&format!(r"(?m)^(?:pub(?:\([^)]*\))? )?fn {name}(?:\(|<)")).unwrap();
    let start = re.find(source).unwrap().start();
    &source[start..start + source[start..].find("\n}\n").unwrap() + 3]
}
#[test]
fn source_pins_no_shared_changed_shims() {
    let old = include_str!("base-memory.rs.txt");
    assert_eq!(
        format!("{:x}", Sha256::digest(old.as_bytes())),
        "8efab26226bc1c23c33ff1309608db45f8f5f99522852dcf0312e6df15e95502"
    );
    let production = include_str!("../../src/memory.rs");
    let reference = include_str!("reference.rs");
    let current = include_str!("current.rs");
    let helpers = include_str!("helpers.rs");
    for name in ["queue_attempt", "flush", "curate"] {
        assert_eq!(
            function(old, name),
            function(reference, name),
            "frozen {name}"
        );
        assert_eq!(
            function(production, name),
            function(current, name),
            "production {name}"
        );
        assert!(
            !helpers.contains(&format!("fn {name}(")),
            "shared changed {name}"
        );
    }
    for name in HELPERS {
        assert_eq!(
            function(old, name),
            function(production, name),
            "unchanged helper {name}"
        );
        assert_eq!(
            function(production, name),
            function(helpers, name),
            "compiled helper {name}"
        );
    }
    // Strip only the three permitted bodies and the new import. This catches
    // accidental edits to feed, ownership, fences, recall and any other helper.
    let strip = |text: &str| {
        let mut text = text.to_owned();
        for name in ["queue_attempt", "flush", "curate"] {
            text = text.replace(function(&text, name), "");
        }
        text.lines()
            .filter(|line| {
                !line.starts_with("use tl3_controller_core::memory_retain")
                    && !line.starts_with("const RECORD_BYTES:")
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(strip(old), strip(production));
}
fn check(entry: Entry) {
    let output = ::std::path::PathBuf::from(
        ::std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
            .unwrap_or_else(|| ::std::env::var_os("TMPDIR").unwrap_or_else(|| ".".into())),
    );
    let regressions = output.join(format!("memory-retain-{entry:?}-proptest-regressions.txt"));
    ::std::fs::write(&regressions, include_str!("proptest-regressions.txt")).unwrap();
    let path = regressions.to_string_lossy().into_owned();
    let mut runner = TestRunner::new(Config {
        cases: 96,
        max_shrink_iters: 512,
        rng_seed: RngSeed::Fixed(0x18_0776_2026),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(Box::leak(
            path.into_boxed_str(),
        )))),
        ..Config::default()
    });
    let tape = prop_oneof![2=>Just(vec![]),3=>(0usize..30,0u8..4).prop_map(|(n,b)|{let mut v=vec![0;n];v.push(b);v}),3=>prop::collection::vec(0u8..4,0..32)];
    let strategy = (
        0u8..24,
        0u8..5,
        0u8..4,
        0u8..3,
        tape,
        prop::collection::vec(0u8..3, 0..16),
        prop::collection::vec(0u8..23, 0..12),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(shape, rows, curations, capacity, reads, writes, http, held, key_error, repeat)| {
                Schedule {
                    shape,
                    rows,
                    curations,
                    capacity,
                    reads,
                    writes,
                    http,
                    held,
                    key_error,
                    repeat,
                }
            },
        );
    let comparisons = ::std::cell::Cell::new(0);
    runner
        .run(&strategy, |s| match io::compare(&s, entry) {
            Ok(n) => {
                comparisons.set(comparisons.get() + n);
                Ok(())
            }
            Err(error) => {
                ::std::fs::write(
                    output.join("memory-retain-failure.json"),
                    serde_json::to_vec_pretty(&s).unwrap(),
                )
                .unwrap();
                Err(TestCaseError::fail(error))
            }
        })
        .unwrap();
    eprintln!(
        "{entry:?}: 96 fixed-seed schedules, {} complete/crash comparisons",
        comparisons.get()
    );
}
#[test]
fn queue_schedules() {
    check(Entry::Queue);
}
#[test]
fn flush_schedules() {
    check(Entry::Flush);
}
#[test]
fn queue_flush_schedules() {
    check(Entry::QueueFlush);
}
fn run(s: &Schedule, e: Entry) -> io::Run {
    io::compare(s, e).unwrap();
    io::run(s, e, true, None)
}
fn calls(r: &io::Run, op: &str) -> usize {
    r.trace.iter().filter(|c| c.0 == op).count()
}
fn queued(r: &io::Run) -> Option<Value> {
    r.trace
        .iter()
        .find(|c| c.0 == "store.queue_memory")
        .map(|c| serde_json::from_str(c.1[1]["content"].as_str().unwrap()).unwrap())
}
#[test]
fn mandatory_queue_qualification_and_persisted_payloads() {
    for shape in 0..24 {
        let r = run(
            &Schedule {
                shape,
                ..Schedule::default()
            },
            Entry::Queue,
        );
        assert!(r.trace.iter().any(|c| c.0 == "fs.open"));
    }
    let success = run(
        &Schedule {
            shape: 20,
            ..Schedule::default()
        },
        Entry::Queue,
    );
    let record = queued(&success).unwrap();
    assert_eq!(record["outcome"], "succeeded");
    assert_eq!(record["head_sha"], "a".repeat(40));
    assert!(record["inputs"].get("tests").is_none());
    assert!(record["inputs"]["repo"].get("materials").is_none());
    assert_eq!(record["activity"]["tests"][0]["action_key"], "a".repeat(64));
    assert_eq!(success.rows.len(), 1);
    assert_eq!(success.effects.len(), 1);
    assert_eq!(success.result, "Ok(0)");
    for (shape, reason) in [
        (4, "worker exit or reply failed"),
        (5, "worker ended without an exit record"),
        (6, "reported failure"),
        (7, "worker exit or reply failed"),
    ] {
        let r = run(
            &Schedule {
                shape,
                ..Schedule::default()
            },
            Entry::Queue,
        );
        let record = queued(&r).unwrap();
        assert_eq!(record["outcome"], "failed");
        assert_eq!(record["reason"], reason);
    }
    let fallback = run(
        &Schedule {
            shape: 3,
            ..Schedule::default()
        },
        Entry::Queue,
    );
    assert_eq!(queued(&fallback).unwrap()["reply"], json!({"final":true}));
    for shape in [10, 15] {
        assert!(
            queued(&run(
                &Schedule {
                    shape,
                    ..Schedule::default()
                },
                Entry::Queue
            ))
            .unwrap()
            .get("head_sha")
            .is_none()
        );
    }
    for shape in [0, 1, 2, 13] {
        assert_eq!(
            calls(
                &run(
                    &Schedule {
                        shape,
                        ..Schedule::default()
                    },
                    Entry::Queue
                ),
                "store.queue_memory"
            ),
            0
        );
    }
    let over = run(
        &Schedule {
            shape: 18,
            ..Schedule::default()
        },
        Entry::Queue,
    );
    assert_eq!(calls(&over, "store.memory_item_bytes"), 1);
    assert_eq!(calls(&over, "store.queue_memory"), 0);
    assert!(over.result.contains("oversized memory record"));
    let flags = run(
        &Schedule {
            shape: 11,
            ..Schedule::default()
        },
        Entry::Queue,
    );
    assert_eq!(calls(&flags, "store.record_memory_curation"), 2);
    assert!(
        flags
            .rows
            .values()
            .any(|v| v["state"] == "hidden" && v["verdict"] == "accepted")
    );
    assert!(flags.rows.values().any(|v| v["verdict"] == "rejected"));
    let conventions = run(
        &Schedule {
            shape: 12,
            ..Schedule::default()
        },
        Entry::Queue,
    );
    assert_eq!(calls(&conventions, "store.queue_memory"), 2);
    assert_eq!(conventions.rows.len(), 2);
}
#[test]
fn mandatory_identity_replay_and_lost_response() {
    let s = Schedule {
        shape: 20,
        repeat: true,
        http: vec![11, 0],
        ..Schedule::default()
    };
    let r = run(&s, Entry::QueueFlush);
    let posts: Vec<_> = r.trace.iter().filter(|c| c.0 == "http.POST").collect();
    assert_eq!(posts.len(), 2);
    assert_eq!(posts[0].1, posts[1].1);
    assert_eq!(r.remote.len(), 1);
    assert_eq!(r.effects.iter().filter(|e| e.0 == "remote.POST").count(), 1);
    assert_eq!(calls(&r, "store.forget_memory"), 1);
    assert_eq!(r.rows.get("ack"), Some(&json!(true)));
    assert!(!r.rows.values().any(|v| v.get("id").is_some()));
    let q = run(
        &Schedule {
            shape: 20,
            repeat: true,
            ..Schedule::default()
        },
        Entry::Queue,
    );
    assert_eq!(calls(&q, "store.queue_memory"), 2);
    assert_eq!(q.rows.len(), 1);
    let id = q.rows.values().next().unwrap()["id"].as_str().unwrap();
    // The real old UUID helper independently checks the changed construction.
    assert_eq!(
        id,
        uuid(&json!(["owner/repo","req-1",3,"key","attempt",{"generation":4}]))
    );
}
#[test]
fn mandatory_http_classification_exact_rows_and_traces() {
    for mode in 0..23 {
        for curation in [false, true] {
            let s = Schedule {
                rows: u8::from(!curation),
                curations: u8::from(curation),
                http: vec![mode],
                ..Schedule::default()
            };
            let r = run(&s, Entry::Flush);
            let success = matches!(mode, 0 | 19 | 20 | 21) || (curation && matches!(mode, 12..=18));
            let park = matches!(mode, 5..=7);
            let key = matches!(mode, 1 | 2);
            assert_eq!(
                calls(
                    &r,
                    if curation {
                        "store.settle_memory_curation"
                    } else {
                        "store.forget_memory"
                    }
                ),
                usize::from(success || (curation && park)),
                "mode {mode} curation {curation}"
            );
            assert_eq!(calls(&r, "store.reject_memory_key"), usize::from(key));
            assert_eq!(
                calls(&r, "store.park_memory"),
                usize::from(!curation && park)
            );
            assert_eq!(calls(&r, "store.memory_delivery_ack"), usize::from(success));
            if curation {
                let v = &r.rows["cur:0"];
                assert_eq!(v["delivered"], json!(success));
                assert_eq!(!v["status"].is_null(), park);
            } else {
                assert_eq!(r.rows.contains_key("out:0"), !success);
            }
            if !success && !park {
                assert!(r.result.starts_with("Err("));
            }
            for t in r.trace.iter().filter(|c| c.0 == "http.timeout") {
                assert_eq!(t.1, 10000);
            }
            for t in r.trace.iter().filter(|c| c.0 == "http.connect_timeout") {
                assert_eq!(t.1, 2000);
            }
        }
    }
    let s = Schedule {
        rows: 3,
        http: vec![5, 0, 8],
        ..Schedule::default()
    };
    let r = run(&s, Entry::Flush);
    assert_eq!(r.rows["out:0"]["parked"], "http");
    assert!(!r.rows.contains_key("out:1"));
    assert!(r.rows.contains_key("out:2"));
    assert_eq!(calls(&r, "store.memory_delivery_ack"), 0);
    assert_eq!(r.remote.len(), 1);
    let s = Schedule {
        rows: 1,
        curations: 1,
        ..Schedule::default()
    };
    let r = run(&s, Entry::Flush);
    let ops: Vec<_> = r.trace.iter().map(|t| t.0.as_str()).collect();
    assert_eq!(
        ops,
        vec![
            "store.expire_parked_memories",
            "store.pending_memory_curations",
            "store.load",
            "store.set_memory_feed_attempt",
            "settings.retain_key",
            "http.timeout",
            "http.connect_timeout",
            "http.client",
            "store.load",
            "store.set_memory_feed_attempt",
            "runtime",
            "http.PATCH",
            "store.settle_memory_curation",
            "store.pending_memory_curations",
            "store.pending_memories",
            "store.load",
            "store.set_memory_feed_attempt",
            "settings.retain_key",
            "http.timeout",
            "http.connect_timeout",
            "http.client",
            "store.load",
            "store.set_memory_feed_attempt",
            "runtime",
            "http.POST",
            "http.body",
            "store.forget_memory",
            "store.pending_memories",
            "store.replay_capacity_memories",
            "store.memory_delivery_ack"
        ]
    );
    assert_eq!(r.result, "Ok(2)");
    assert_eq!(r.rows["cur:0"]["delivered"], true);
    assert!(!r.rows.contains_key("out:0"));
}
#[test]
fn mandatory_batches_holds_empty_wakes_and_crash_rows() {
    let r = run(&Schedule::default(), Entry::Flush);
    assert_eq!(r.result, "Ok(0)");
    assert_eq!(
        r.trace.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(),
        vec![
            "store.expire_parked_memories",
            "store.pending_memory_curations",
            "store.pending_memories"
        ]
    );
    assert!(r.rows.is_empty());
    for (rows, curations) in [(1, 0), (0, 1)] {
        let r = run(
            &Schedule {
                rows,
                curations,
                held: true,
                ..Schedule::default()
            },
            Entry::Flush,
        );
        assert_eq!(calls(&r, "http.POST") + calls(&r, "http.PATCH"), 0);
        assert_eq!(r.rows["feed"]["held"], true);
        assert_eq!(calls(&r, "store.set_memory_feed_attempt"), 1);
    }
    let r = run(
        &Schedule {
            rows: 65,
            capacity: 2,
            ..Schedule::default()
        },
        Entry::Flush,
    );
    assert_eq!(r.result, "Ok(67)");
    assert_eq!(calls(&r, "http.POST"), 67);
    assert_eq!(r.remote.len(), 67);
    assert_eq!(calls(&r, "store.pending_memories"), 5);
    assert_eq!(calls(&r, "store.replay_capacity_memories"), 2);
    for call in r.trace.iter().filter(|t| t.0 == "store.pending_memories") {
        assert!(call.2.as_array().unwrap().len() <= 64);
    }
    for call in r.trace.iter().filter(|t| t.0 == "http.POST") {
        assert_eq!(call.1["body"]["items"].as_array().unwrap().len(), 1);
    }
    let r = run(
        &Schedule {
            curations: 65,
            ..Schedule::default()
        },
        Entry::Flush,
    );
    assert_eq!(r.result, "Ok(65)");
    assert_eq!(calls(&r, "http.PATCH"), 65);
    assert_eq!(calls(&r, "store.pending_memory_curations"), 3);
    let r = run(
        &Schedule {
            shape: 23,
            ..Schedule::default()
        },
        Entry::Flush,
    );
    assert!(!r.rows.contains_key("expired"));
    assert!(r.rows.contains_key("recent"));
    assert_eq!(calls(&r, "http.PATCH"), 0);
    let s = Schedule {
        rows: 1,
        ..Schedule::default()
    };
    let full = io::run(&s, Entry::Flush, true, None);
    assert!(full.effects.iter().any(|e| e.0 == "store.forget_memory"));
    // HTTP effects are not store-write indices: locate deletion from trace instead.
    let index = full
        .trace
        .iter()
        .take_while(|t| t.0 != "store.forget_memory")
        .filter(|t| {
            matches!(
                t.0.as_str(),
                "store.expire_parked_memories" | "store.set_memory_feed_attempt"
            )
        })
        .count();
    let before = io::run(&s, Entry::Flush, true, Some((index, false)));
    let after = io::run(&s, Entry::Flush, true, Some((index, true)));
    assert!(before.rows.contains_key("out:0"));
    assert!(!after.rows.contains_key("out:0"));
    assert_eq!(before.remote, after.remote);
    assert_eq!(before.result, "crash");
    assert_eq!(after.result, "crash");
}

#[test]
fn generated_http_outcome_matrix() {
    let output = ::std::path::PathBuf::from(
        ::std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
            .unwrap_or_else(|| ::std::env::var_os("TMPDIR").unwrap_or_else(|| ".".into())),
    );
    let mut comparisons = 0;
    for mode in 0..23u8 {
        let regression = output.join(format!("memory-http-{mode}-regressions.txt"));
        ::std::fs::write(&regression, include_str!("proptest-regressions.txt")).unwrap();
        let mut runner = TestRunner::new(Config {
            cases: 4,
            max_shrink_iters: 512,
            rng_seed: RngSeed::Fixed(0x18_0776_2026),
            failure_persistence: Some(Box::new(FileFailurePersistence::Direct(Box::leak(
                regression.to_string_lossy().into_owned().into_boxed_str(),
            )))),
            ..Config::default()
        });
        let strategy = (1u8..5, prop::collection::vec(0u8..23, 0..8), any::<bool>());
        runner
            .run(&strategy, |(rows, tail, repeat)| {
                let mut http = vec![mode];
                http.extend(tail);
                let schedule = Schedule {
                    rows,
                    http,
                    repeat,
                    ..Schedule::default()
                };
                match io::compare(&schedule, Entry::Flush) {
                    Ok(n) => {
                        comparisons += n;
                        Ok(())
                    }
                    Err(error) => {
                        ::std::fs::write(
                            output.join("memory-http-minimized-failure.json"),
                            serde_json::to_vec_pretty(&schedule).unwrap(),
                        )
                        .unwrap();
                        Err(TestCaseError::fail(error))
                    }
                }
            })
            .unwrap();
    }
    eprintln!("HTTP matrix: 92 fixed-seed schedules, {comparisons} complete/crash comparisons");
}
