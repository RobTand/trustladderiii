//! H19 extraction equivalence, over changing reads and every before/after-write crash.
pub use tl3_controller::{Error, Result, executor, findings};
#[path = "../../src/adapters/prismabuild/decide.rs"]
pub mod decide;
mod extracted;
mod io;
mod reference;

pub mod tl3_store {
    pub use ::tl3_store::ExternalWait;
    pub struct Store;
    pub struct Snapshot {
        pub pending_effects: Vec<Effect>,
        pub rows: Vec<Row>,
    }
    pub struct Effect {
        pub kind: String,
        pub payload: serde_json::Value,
    }
    pub struct Row {
        pub table: String,
        pub value: serde_json::Value,
    }
    impl Store {
        pub fn load(&mut self) -> crate::Result<Snapshot> {
            crate::io::sample("store.load");
            Ok(Snapshot {
                pending_effects: vec![],
                rows: vec![],
            })
        }
    }
}
pub mod snapshot {
    pub struct World {
        pub nodes: Vec<Node>,
    }
    pub struct Node {
        pub phase: Phase,
    }
    pub struct Phase(bool);
    impl Phase {
        pub fn is_closed(&self) -> bool {
            self.0
        }
    }
    pub fn decode(_snapshot: &super::tl3_store::Snapshot) -> crate::Result<World> {
        let closed = crate::io::sample("snapshot.decode").is_multiple_of(2);
        Ok(World {
            nodes: vec![Node {
                phase: Phase(closed),
            }],
        })
    }
}
use io::{Delivery, Job, State};
use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner};
use std::collections::BTreeMap;

fn delivery(withdraw: bool) -> Delivery {
    serde_json::from_value(serde_json::json!({
        "key":"checks-0", "epoch":1,
        "output":{"Effect":{"node":0,"kind":if withdraw {"WithdrawJob"}else{"SubmitChecks"},"seq":1,"head":null}},
        "generation":2, "job_generation":1,"head_sha":"b".repeat(40),
        "repository":{"owner":"RobTand","name":"tl3-sandbox","installation":1},
        "issue":730,"pull_request":null,"paths":[],"attempt":1,"round":0,
        "checks":{"repository":"/served","base_sha":"a".repeat(40),"plan":[]}
    })).unwrap()
}
fn quiet_crashes() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !info.payload().is::<io::Crash>() {
                previous(info);
            }
        }));
    });
}

fn initial(script: &[u8], binding: bool, done: bool, wait: bool) -> State {
    let d = delivery(false);
    let mut files = BTreeMap::from([
        ("/records".into(), vec![]),
        ("/records/0".into(), vec![]),
        ("/records/epoch.fence".into(), b"0".to_vec()),
    ]);
    let key = d
        .key
        .bytes()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if binding {
        let mut old = d.clone();
        old.generation = 1;
        let job = Job {
            action: "a".repeat(64).parse().unwrap(),
            run: old.checks.clone().unwrap(),
            delivery: old,
            changed_paths: vec![],
            selected_tests: None,
            always_tests: vec![],
        };
        files.insert(
            format!("/records/0/{key}.job"),
            serde_json::to_vec(&job).unwrap(),
        );
    }
    if done {
        files.insert(
            format!("/records/0/{key}.done"),
            serde_json::to_vec(&io::Completion::confirmed(vec![])).unwrap(),
        );
    }
    if wait {
        let sub = io::WaitSubscription {
            delivery: d,
            wait: tl3_store::ExternalWait {
                upstream: "prismabuild".into(),
                key: "a".repeat(64),
            },
        };
        files.insert(
            format!("/records/0/{key}.wait"),
            serde_json::to_vec(&sub).unwrap(),
        );
    }
    State {
        calls: vec![],
        synced: files.clone(),
        persisted: files.clone(),
        files,
        reads: 0,
        writes: 0,
        script: script.to_vec(),
        crash: usize::MAX,
    }
}
#[derive(Debug, PartialEq, Eq)]
struct Observed {
    result: String,
    calls: Vec<String>,
    effects: BTreeMap<String, Vec<u8>>,
    persisted: BTreeMap<String, Vec<u8>>,
    writes: usize,
}
fn replay(old: bool, mut state: State, op: u8, wait_mode: u8) -> Observed {
    quiet_crashes();
    let crash = state.crash;
    state.calls.clear();
    state.writes = 0;
    state.reads = 0;
    io::IO.with(|io| *io.borrow_mut() = state);
    let wait = tl3_store::ExternalWait {
        upstream: if wait_mode == 2 {
            "other"
        } else {
            "prismabuild"
        }
        .into(),
        key: if wait_mode == 3 {
            "invalid".into()
        } else {
            "a".repeat(64)
        },
    };
    let d = delivery(op & 8 != 0);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if old {
            reference::run(op, &d, (wait_mode != 0).then_some(&wait))
        } else {
            extracted::run(op, &d, (wait_mode != 0).then_some(&wait))
        }
    }));
    let result = match result {
        Ok(value) => value,
        Err(value) if value.is::<io::Crash>() => format!("crash:{crash}"),
        Err(value) => std::panic::resume_unwind(value),
    };
    io::IO.with(|io| {
        let io = io.borrow();
        Observed {
            result,
            calls: io.calls.clone(),
            effects: io.files.clone(),
            persisted: io.persisted.clone(),
            writes: io.writes,
        }
    })
}
fn check(
    script: &[u8],
    op: u8,
    flags: u8,
    wait: u8,
) -> std::result::Result<(), proptest::test_runner::TestCaseError> {
    let state = initial(script, flags & 1 != 0, flags & 2 != 0, flags & 4 != 0);
    let expected = replay(true, state.clone(), op, wait);
    let actual = replay(false, state.clone(), op, wait);
    prop_assert_eq!(&expected, &actual, "uninterrupted op={}", op);
    for crash in 0..expected.writes * 2 {
        let mut state = state.clone();
        state.crash = crash;
        let expected = replay(true, state.clone(), op, wait);
        let actual = replay(false, state, op, wait);
        prop_assert_eq!(&expected, &actual, "crash={} op={}", crash, op);
    }
    Ok(())
}
#[test]
fn differential_io_schedules() {
    let output = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target"));
    std::fs::create_dir_all(&output).unwrap();
    let regressions = output.join("prismabuild_schedule-regressions.txt");
    std::fs::write(&regressions, include_str!("io_schedule-regressions.txt")).unwrap();
    let path = Box::leak(regressions.to_string_lossy().into_owned().into_boxed_str());
    let mut runner = TestRunner::new(Config {
        cases: 128,
        max_shrink_iters: 2048,
        rng_seed: RngSeed::Fixed(73_019),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(path))),
        ..Config::default()
    });
    runner
        .run(
            &(
                prop::collection::vec(any::<u8>(), 1..32),
                0u8..16,
                0u8..8,
                0u8..4,
            ),
            |(script, op, flags, wait)| check(&script, op, flags, wait),
        )
        .unwrap();
}
#[test]
fn every_protocol_path_and_crash_boundary() {
    for op in 0..16 {
        for flags in 0..8 {
            for wait in 0..4 {
                check(&[0], op, flags, wait).unwrap();
                check(&[4, 1, 0, 2, 0, 3, 4, 0, 1], op, flags, wait).unwrap();
            }
        }
    }
}
#[test]
fn replay_bodies_are_the_production_bodies() {
    let source = include_str!("../../src/adapters/prismabuild.rs");
    let replay = include_str!("extracted.rs");
    let mut count = 0;
    for body in replay.split("// BEGIN BODY\n").skip(1) {
        let body = body.split("\n// END BODY").next().unwrap();
        assert!(
            source.contains(body),
            "replay.py must refresh this production body: {}",
            body.lines().next().unwrap()
        );
        count += 1;
    }
    assert_eq!(count, 19);
}

/// Once by action key: every before/after-write crash restarts using only durable
/// bytes and the upstream's surviving authenticated record, never a cached result.
#[test]
fn publication_is_once_across_every_crash() {
    let state = initial(&[0], false, false, false);
    let uninterrupted = replay(false, state.clone(), 0, 0);
    assert!(
        uninterrupted
            .calls
            .iter()
            .any(|c| c.starts_with("pb.publish:"))
    );
    for crash in 0..uninterrupted.writes * 2 {
        let mut first = state.clone();
        first.crash = crash;
        let first = replay(false, first, 0, 0);
        let mut recovered = state.clone();
        recovered.files = first.persisted.clone();
        // Directories survive their original mkdir/parent-fsync boundary.
        recovered.files.entry("/records".into()).or_default();
        recovered.files.entry("/records/0".into()).or_default();
        recovered.synced = recovered.files.clone();
        recovered.persisted = first.persisted.clone();
        let second = replay(false, recovered, 0, 0);
        let publications = first
            .calls
            .iter()
            .chain(&second.calls)
            .filter(|c| c.starts_with("pb.publish:"))
            .count();
        // A crash before the write records its attempted call, but publishes no work.
        let landed_first = first.persisted.keys().any(|k| k.starts_with("pb:"));
        if landed_first {
            assert_eq!(publications, 1, "crash {crash} repeated publication");
        }
        assert!(
            second.persisted.keys().any(|k| k.starts_with("pb:")),
            "crash {crash} lost publication"
        );
    }
}
