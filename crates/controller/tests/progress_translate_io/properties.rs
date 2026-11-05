use super::*;
use io::{Schedule, compare};
use proptest::{
    prelude::*,
    test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
};

fn allowance() -> Value {
    json!({"delivery":{"key":"step","epoch":1,"output":{"Effect":{"node":0,"seq":0,"kind":"Enqueue","head":null}},"generation":0,"head_sha":null,"repository":{"owner":"owner","name":"repo","installation":1},"issue":1,"pull_request":100,"paths":[],"attempt":0,"round":0},
        "due_ms":9000,"progress":null,"progress_ms":0,"quota_ms":null,"stopping":false,"incident":null,
        "unserved_since_ms":100,"ladder_out_noticed":true})
}
fn progress(checkpoint: &str) -> Observation {
    Observation::Progress {
        key: "step".into(),
        checkpoint: checkpoint.into(),
    }
}
fn quota(model: Option<&str>, reset_ms: u64) -> Observation {
    Observation::Quota {
        key: "step".into(),
        reset_ms,
        model: model.map(str::to_owned),
    }
}
fn schedule(observation: Observation) -> Schedule {
    Schedule {
        rows: vec![("allowances".into(), "step".into(), allowance())],
        observations: vec![observation],
        clocks: vec![10000, 12000],
        write_results: vec![],
    }
}
fn stored(run: &io::Run, path: &str) -> Value {
    serde_json::from_slice(&run.files[path]).unwrap()
}
fn ops(run: &io::Run) -> Vec<&str> {
    run.trace.iter().map(|v| v[0].as_str().unwrap()).collect()
}
/// State-table:796,800; Q43 context. Clearing a wait on a new checkpoint preserves existing behavior.
#[test]
fn mandatory_progress_new_same_and_external_wait() {
    for checkpoint in ["", "tool-result"] {
        for waiting in [false, true] {
            let mut s = schedule(progress(checkpoint));
            s.rows[0].2["wait_ended"] = json!(true);
            if waiting {
                s.rows[0].2["external_wait"] =
                    json!({"upstream":"prismabuild","key":"outside-job"});
            }
            let r = compare(&s);
            assert_eq!(r.results, [json!(["ok", null])]);
            let a = stored(&r, "allowances/step");
            assert_eq!(a["progress"], checkpoint);
            assert_eq!(a["progress_ms"], 10000);
            assert_eq!(a["due_ms"], 11000);
            assert_eq!(a["unserved_since_ms"], Value::Null);
            assert_eq!(a["ladder_out_noticed"], false);
            assert_eq!(a["wait_ended"], false);
            assert_eq!(a["external_wait"], Value::Null);
            assert_eq!(
                ops(&r),
                if waiting {
                    vec!["translate", "row.push", "clock", "row.push", "store.commit"]
                } else {
                    vec!["translate", "clock", "row.push", "store.commit"]
                }
            );
            assert_eq!(r.files.contains_key("retiring_waits/step"), waiting);
            if waiting {
                assert_eq!(r.buffered[0][1], "retiring_waits");
            }
            s.rows[0].2["progress"] = json!(checkpoint);
            let unchanged = compare(&s);
            assert_eq!(ops(&unchanged), ["translate", "store.commit"]);
            assert_eq!(stored(&unchanged, "allowances/step"), s.rows[0].2);
        }
    }
}
/// Q53, state-table:277. Equal/older model reset suppression and replay are preserved implementation details.
#[test]
fn mandatory_all_quota_model_reset_and_replay_shapes() {
    for model in [None, Some(""), Some("model-a"), Some("model-b")] {
        for previous in [None, Some(0), Some(99), Some(100), Some(101)] {
            let mut s = schedule(quota(model, 100));
            if let Some(previous) = previous {
                s.rows.push((
                    "model_quotas".into(),
                    model.unwrap_or("unrelated").into(),
                    json!({"reset_ms":previous}),
                ));
            }
            let r = compare(&s);
            let record = model.is_some() && previous.is_none_or(|p| p < 100);
            assert_eq!(r.buffered.len(), 1 + usize::from(record));
            assert_eq!(ops(&r).iter().filter(|op| **op == "clock").count(), 2);
            let a = stored(&r, "allowances/step");
            assert_eq!(a["quota_ms"], 100);
            assert_eq!(a["last_quota_ms"], 100);
            assert_eq!(a["last_quota_model"], json!(model));
            assert_eq!(a["unserved_since_ms"], 100);
            assert_eq!(a["due_ms"], 12000);
            if record {
                assert_eq!(r.buffered[0][1], "model_quotas");
            }
            s.rows[0].2["last_quota_ms"] = json!(100);
            s.rows[0].2["last_quota_model"] = json!(model);
            let replay = compare(&s);
            assert_eq!(replay.buffered.len(), usize::from(record));
            assert_eq!(stored(&replay, "allowances/step"), s.rows[0].2);
            assert!(!ops(&replay).contains(&"clock"));
            s.rows[0].2["last_quota_model"] = json!("different");
            assert!(ops(&compare(&s)).contains(&"clock"));
        }
    }
    for reset in [0, u64::MAX] {
        let mut s = schedule(quota(Some("model-a"), reset));
        s.rows[0].2["unserved_since_ms"] = Value::Null;
        let r = compare(&s);
        assert_eq!(stored(&r, "allowances/step")["unserved_since_ms"], 10000);
        assert_eq!(stored(&r, "model_quotas/model-a")["reset_ms"], reset);
        assert_eq!(r.trace[1][0], "row.push");
        assert_eq!(r.trace[2], json!(["clock", 10000]));
        assert_eq!(r.trace[3], json!(["clock", 12000]));
    }
}
/// Preserved existing behavior: all upstream allowance projections and saturating deadlines.
#[test]
fn mandatory_progress_upstreams_and_saturating_deadline() {
    for kind in [
        "StopRun",
        "SubmitChecks",
        "PrepareWorktree",
        "OpenPr",
        "Enqueue",
    ] {
        let mut s = schedule(progress("tool"));
        s.rows[0].2["delivery"]["output"]["Effect"]["kind"] = json!(kind);
        s.clocks = vec![u64::MAX];
        let r = compare(&s);
        assert_eq!(stored(&r, "allowances/step")["progress_ms"], u64::MAX);
        assert_eq!(stored(&r, "allowances/step")["due_ms"], u64::MAX);
        assert_eq!(ops(&r), ["translate", "clock", "row.push", "store.commit"]);
    }
    for reset in [99, 101] {
        let mut s = schedule(quota(Some("model-a"), reset));
        s.rows[0].2["last_quota_ms"] = json!(100);
        s.rows[0].2["last_quota_model"] = json!("model-a");
        let r = compare(&s);
        let changed = reset > 100;
        assert_eq!(
            stored(&r, "allowances/step")["quota_ms"],
            if changed { json!(reset) } else { Value::Null }
        );
        assert_eq!(
            ops(&r).iter().filter(|op| **op == "clock").count(),
            if changed { 2 } else { 0 }
        );
        assert_eq!(stored(&r, "model_quotas/model-a")["reset_ms"], reset);
    }
}
/// Preserved existing behavior: missing/malformed rows and transaction errors, no kernel fact.
#[test]
fn mandatory_absent_malformed_and_write_errors() {
    for input in [
        progress("next"),
        quota(None, 100),
        quota(Some("model-a"), 100),
    ] {
        let mut s = schedule(input);
        s.rows.clear();
        let missing = compare(&s);
        assert_eq!(missing.results, [json!(["ok", null])]);
        assert!(!ops(&missing).contains(&"clock"));
        s.rows
            .push(("allowances".into(), "step".into(), json!({"invalid":true})));
        let invalid = compare(&s);
        assert_eq!(invalid.results[0][0], "error");
        assert!(!ops(&invalid).contains(&"store.commit"));
        assert!(!invalid.files.contains_key("model_quotas/model-a"));
        s.rows[0].2 = allowance();
        for mode in 1..=3 {
            s.write_results = vec![mode];
            let failed = compare(&s);
            assert_eq!(failed.results[1], json!(["commit.error", mode]));
            assert_eq!(stored(&failed, "allowances/step"), allowance());
        }
    }
    let mut s = schedule(quota(Some("model-a"), 100));
    s.rows.push((
        "model_quotas".into(),
        "model-a".into(),
        json!({"reset_ms":"malformed"}),
    ));
    assert_eq!(
        stored(&compare(&s), "model_quotas/model-a")["reset_ms"],
        100
    );
}
/// #759/state-machine:507: a served checkpoint resets the unserved run; two quota clocks stay distinct.
#[test]
fn mandatory_sequence_and_crash_atomicity() {
    let mut s = schedule(quota(Some("model-a"), 100));
    s.rows[0].2["unserved_since_ms"] = Value::Null;
    s.observations.extend([
        progress("tool"),
        quota(Some("model-a"), 100),
        quota(Some("model-b"), 200),
    ]);
    s.clocks = vec![1000, 2000, 3000, 4000, 5000];
    let r = compare(&s);
    assert_eq!(r.results.len(), 4);
    assert_eq!(r.writes, 4);
    let a = stored(&r, "allowances/step");
    assert_eq!(a["last_quota_model"], "model-b");
    assert_eq!(a["unserved_since_ms"], 4000);
    assert_eq!(a["due_ms"], 5000);
    let mut s = schedule(progress("new"));
    s.rows[0].2["external_wait"] = json!({"upstream":"prismabuild","key":"outside-job"});
    let before = io::crash(&s, false);
    assert_eq!(stored(&before, "allowances/step"), s.rows[0].2);
    assert!(!before.files.contains_key("retiring_waits/step"));
    let after = io::crash(&s, true);
    assert_eq!(stored(&after, "allowances/step")["progress"], "new");
    assert!(after.files.contains_key("retiring_waits/step"));
}

#[test]
fn generated_changing_clocks_shapes_sequences_and_crashes() {
    let output = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .or_else(|| std::env::var_os("TEST_TMPDIR"))
        .expect("writable test output");
    let path = std::path::PathBuf::from(output).join("g2d-proptest-regressions.txt");
    std::fs::write(&path, include_str!("proptest-regressions.txt")).unwrap();
    let persistence = Box::leak(path.to_str().unwrap().to_owned().into_boxed_str());
    let config = Config {
        cases: 256,
        rng_seed: RngSeed::Fixed(0x00d2_0475_2026),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(persistence))),
        ..Config::default()
    };
    let strategy = (
        any::<u8>(),
        prop::collection::vec((any::<u8>(), any::<u16>()), 1..12),
        prop::collection::vec(0u64..1_000_000, 0..24),
        prop::collection::vec(0u8..4, 0..12),
    );
    let mut runner = TestRunner::new(config);
    runner
        .run(&strategy, |(shape, entries, clocks, writes)| {
            let mut s = schedule(progress(""));
            s.clocks = clocks;
            s.write_results = writes;
            if shape & 1 != 0 {
                s.rows[0].2["progress"] = json!("");
            }
            if shape & 2 != 0 {
                s.rows[0].2["external_wait"] =
                    json!({"upstream":"prismabuild","key":"outside-job"});
            }
            if shape & 4 != 0 {
                s.rows[0].2["last_quota_ms"] = json!(0);
                s.rows[0].2["last_quota_model"] = json!("model-a");
            }
            if shape & 8 != 0 {
                s.rows[0].2["unserved_since_ms"] = Value::Null;
            }
            if shape & 16 != 0 {
                s.rows.push((
                    "model_quotas".into(),
                    "model-a".into(),
                    json!({"reset_ms":0}),
                ));
            }
            if shape & 32 != 0 {
                s.rows.clear();
            }
            if shape & 64 != 0 {
                s.rows
                    .push(("allowances".into(), "unrelated".into(), json!(false)));
            }
            if shape & 128 != 0 && !s.rows.is_empty() {
                s.rows[0].2 = json!({"invalid":true});
            }
            s.observations = entries
                .into_iter()
                .map(|(tag, reset)| match tag % 6 {
                    0 => progress(""),
                    1 => progress("tool"),
                    2 => quota(None, u64::from(reset)),
                    3 => quota(Some(""), u64::from(reset)),
                    4 => quota(Some("model-a"), u64::from(reset)),
                    _ => quota(Some("model-b"), u64::from(reset)),
                })
                .collect();
            s.observations.extend([
                quota(Some("model-a"), 0),
                quota(Some("model-a"), 0),
                progress(""),
                progress("tool"),
                progress("tool"),
                quota(None, 1),
                quota(Some(""), 1),
                quota(Some("model-b"), 1),
            ]);
            compare(&s);
            Ok(())
        })
        .unwrap();
    println!(
        "G2d: 256 generated schedules plus committed minimized-seed replay; {} comparisons including crashes before/after every attempted commit",
        io::comparisons()
    );
}
