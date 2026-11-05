use super::super::Check;
use super::*;
use proptest::prelude::*;
use serde_json::{Value, json};

#[path = "before.rs"]
mod before;

mod campaign_case;
mod producer_cases;

fn fixture(base: &str, head: &str, passed: bool, pre_existing: bool) -> (Job, Value) {
    let job: Job = serde_json::from_value(json!({
        "action": "a".repeat(64),
        "delivery": {"key": "checks:822", "epoch": 1,
            "output": {"Effect": {"node": 0, "kind": "SubmitChecks", "seq": 1, "head": null}},
            "generation": 1, "head_sha": "b".repeat(40),
            "repository": {"owner": "fixture", "name": "repo", "installation": 1},
            "issue": 822, "pull_request": null, "paths": [], "attempt": 1, "round": 0},
        "run": {"repository": "/served", "base_sha": "a".repeat(40),
            "plan": [{"name": "regression", "kind": "regression", "tests": ["tests/test_x.py::test_x"]}]},
        "changed_paths": [], "always_tests": [], "selected_tests": null,
    })).unwrap();
    let report = json!({"schema": "tl3.checks.v1", "effect_key": "checks:822",
        "base": "a".repeat(40), "head": "b".repeat(40), "reviewed": null,
        "results": [{"name": "regression", "kind": "regression", "custody": "in-process",
            "verdict": if base == "no-report" || head == "no-report" { "infrastructure-failure" }
                else if passed { "pass" } else { "fail" },
            "tests": [{"test": "tests/test_x.py::test_x", "base": base, "head": head,
                "passed": passed, "pre_existing": pre_existing, "message": ""}]}]});
    (job, report)
}

fn raw(report: &Value) -> Vec<u8> {
    format!("TL3_CHECKS {report}\n").into_bytes()
}

fn ending(raw: &[u8]) -> Ending {
    serde_json::from_value(json!({
        "action_key": "a".repeat(64), "status": "executed", "succeeded": true, "exit_code": 0,
        "result": {"verified": true, "receipt_sha256": "c".repeat(64), "stdout": {
            "path": "/recorded/stdout", "bytes": raw.len(), "sha256": format!("{:x}", Sha256::digest(raw))}},
    })).unwrap()
}

fn evaluate(job: &Job, report: &Value) -> CheckEvidence {
    let raw = raw(report);
    evidence_with_read(ending(&raw), job, |_| Ok(raw.clone())).unwrap()
}

/// #822: use real producer output after late parse rejection and selected retries.
#[test]
fn producer_overrides_preserve_retained_phases() {
    let cases: Vec<Value> = serde_json::from_str(producer_cases::CASES).unwrap();
    for selected in [false, true] {
        let (mut job, mut report) = fixture("passed", "passed", true, false);
        if selected {
            job.selected_tests = Some(1);
            report["results"]
                .as_array_mut()
                .unwrap()
                .push(cases[1].clone());
        } else {
            report["results"][0] = cases[0].clone();
        }
        let bytes = raw(&report);
        recorded_io::reset(bytes.clone(), 0);
        let old = normalized(before::run(ending(&bytes), &job));
        let new = normalized(Ok(evaluate(&job, &report)));
        assert_eq!(new, old, "producer override selected={selected}");
        assert_eq!(new["class"], Value::Null);
        assert_eq!(new["pass"], false);
        assert!(!new["results"].as_array().unwrap().is_empty());
    }
}

#[test]
fn disagreement_uses_the_confirmed_harness_class() {
    let (job, report) = fixture("skipped", "passed", true, false);
    assert!(
        parse(&raw(&report), &job).is_some(),
        "shape/identity validation must not swallow a judge disagreement"
    );
    let (report, pass, results, failed, class) = evaluate(&job, &report);
    assert!(!report && !pass && results.is_empty() && failed.is_empty());
    assert_eq!(class, Some(HARNESS));
    assert_eq!(JUDGE_MISMATCH_CLASS, Some(HARNESS));
}

async fn mismatch_completion(base: &str, python_passed: bool) {
    let (mut job, report) = fixture(base, "passed", python_passed, false);
    job.delivery.checks = Some(job.run.clone());
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("stdout");
    let bytes = raw(&report);
    std::fs::write(&log, &bytes).unwrap();
    let response = json!({"schema":"prismabuild.endings.v1","complete":true,
        "endings":[{"action_key":job.action,"status":"executed","succeeded":true,"exit_code":0,
            "result":{"verified":true,"receipt_sha256":"c".repeat(64),"stdout":{
                "path":log,"bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(&bytes))}}}]});
    let path = directory.path().join("endings.json");
    std::fs::write(&path, serde_json::to_vec(&response).unwrap()).unwrap();
    let tool = Tool {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "cat \"$1\"".into(),
            "pbwait-fixture".into(),
            path.into_os_string(),
        ],
    };
    let completion = super::ending(&tool, &job, false, &tl3_prismabuild::RecordRead::Absent)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(completion.ack, tl3_kernel::Ack::Confirmed));
    assert_eq!(completion.failure_class.as_deref(), Some(HARNESS));
    let [
        Observation::Checks {
            report,
            pass,
            results,
            failed_tests,
            ..
        },
    ] = completion.observations.as_slice()
    else {
        panic!("mismatch must produce exactly the confirmed Checks observation")
    };
    assert!(!report && !pass && results.is_empty() && failed_tests.is_empty());
}

/// Q51/#822: neither a rejected Python pass nor its findings may become request failure.
#[tokio::test]
async fn mismatch_completion_rust_rejects_python_pass() {
    mismatch_completion("skipped", true).await;
}

/// Q51/#822: neither a rejected Python failure nor the Rust pass may escape the mismatch.
#[tokio::test]
async fn mismatch_completion_rust_accepts_python_fail() {
    mismatch_completion("passed", false).await;
}

#[test]
fn controller_rejects_false_negative_and_wrong_pre_existing() {
    let (job, false_negative) = fixture("passed", "passed", false, false);
    assert_eq!(evaluate(&job, &false_negative).4, Some(HARNESS));
    let (_, wrong_pre_existing) = fixture("added-name", "passed", true, true);
    assert_eq!(evaluate(&job, &wrong_pre_existing).4, Some(HARNESS));
    let (_, correct) = fixture("added-name", "passed", true, false);
    assert!(evaluate(&job, &correct).1);
}

/// #822/execution.py:681: changed-test refusal replaces even an infrastructure verdict.
#[test]
fn changed_test_refusal_preserves_its_failure_over_no_report() {
    let (mut job, mut report) = fixture("no-report", "passed", false, false);
    report["results"][0]["verdict"] = json!("fail");
    job.changed_paths = vec!["tests/test_x.py".into()];
    let (reported, passed, results, failed, class) = evaluate(&job, &report);
    assert!(reported && !passed && !results.is_empty() && !failed.is_empty());
    assert_eq!(class, None);

    // Without the changed-path fact, infrastructure cannot silently become a test failure.
    job.changed_paths.clear();
    assert_eq!(evaluate(&job, &report).4, Some(HARNESS));
}

fn phases(call_status: &str, assertion: bool) -> Value {
    json!({"test": "tests/test_x.py::test_x", "setup": {"outcome": "passed", "assertion": false, "exception": ""},
        "call": {"outcome": call_status, "assertion": assertion, "exception": ""},
        "teardown": {"outcome": "passed", "assertion": false, "exception": ""}})
}

#[test]
fn controller_folds_phases_without_inventing_missing_name_facts() {
    let (job, mut report) = fixture("added-name", "passed", true, false);
    report["results"][0]["tests"][0]["phases"] = json!({
        "base": phases("failed", false), "head": phases("passed", false)});
    assert!(evaluate(&job, &report).1);
    report["results"][0]["tests"][0]["phases"]["head"] = phases("failed", true);
    assert_eq!(evaluate(&job, &report).4, Some(HARNESS));
}

#[test]
fn changed_test_override_is_not_reconstructed_from_clipped_message() {
    let (mut job, report) = fixture("passed", "passed", false, false);
    job.changed_paths.push("tests/test_x.py".into());
    let result = evaluate(&job, &report);
    assert!(result.0 && !result.1 && result.4.is_none());
}

#[test]
fn known_missing_red_match_is_a_judge_disagreement() {
    let (mut job, mut report) = fixture("assertion", "passed", true, false);
    let declaration = json!({"subjects": [], "observer_support": [], "red_message": "expected"});
    job.run.plan[0].kind = Kind::Demonstrating;
    job.run.plan[0].proof = Some(serde_json::from_value(declaration.clone()).unwrap());
    report["results"][0]["kind"] = json!("demonstrating");
    report["results"][0]["proof"] = json!({"rule": "subject-observer-v1", "mode": "head-observer",
        "declaration": declaration, "subjects": {}, "support_sha256": {},
        "observer_sha256": {"tests/test_x.py::test_x": "c".repeat(64)},
        "source_base_tree_sha256": "d".repeat(64), "base_tree_sha256": "d".repeat(64),
        "head_tree_sha256": "e".repeat(64), "red_message_matches": {}});
    assert_eq!(evaluate(&job, &report).4, Some(HARNESS));
    report["results"][0]["proof"]["red_message_matches"] =
        json!({"tests/test_x.py::test_x": "message"});
    assert!(evaluate(&job, &report).1);
    // Previous-release receipts omit this field; do not infer a negative receipt.
    report["results"][0]["proof"]
        .as_object_mut()
        .unwrap()
        .remove("red_message_matches");
    assert!(evaluate(&job, &report).1);
}

#[test]
fn retained_boundary_requires_positive_proof_identity() {
    let (mut job, mut report) = fixture("added-name", "passed", true, false);
    job.run.plan[0].kind = Kind::Demonstrating;
    job.run.plan[0].proof = Some(
        serde_json::from_value(json!({
            "subjects": [], "observer_support": [], "red_message": "expected",
        }))
        .unwrap(),
    );
    report["results"][0]["kind"] = json!("demonstrating");
    assert!(parse(&raw(&report), &job).is_none());
}

#[test]
fn retained_boundary_checks_campaign_flags_and_forced_heads() {
    let (mut job, mut report) = fixture("error", "passed", true, false);
    let declaration = json!({"node": "tests/test_x.py::test_x",
        "exception": "builtins.TimeoutError", "phase": "call", "repeat": 1, "parallel": 0});
    job.run.plan[0].kind = Kind::Demonstrating;
    job.run.plan[0].declared_symptom = Some(serde_json::from_value(declaration.clone()).unwrap());
    report["results"][0]["kind"] = json!("demonstrating");
    report["results"][0]["declared_symptom"] = declaration;
    report["results"][0]["verdict"] = json!("fail");
    assert_eq!(evaluate(&job, &report).4, Some(HARNESS));

    report["results"][0]["verdict"] = json!("pass");
    report["results"][0]["forcing"] =
        json!({"symptom": "timeout", "environment": {"OTHER": "1"}, "patches": []});
    let typed: Report = serde_json::from_value(report.clone()).unwrap();
    assert!(!rejudged(&typed, &job));
    report["results"][0]["tests"][0]["head_forced"] = json!("passed");
    let typed: Report = serde_json::from_value(report).unwrap();
    assert!(rejudged(&typed, &job));
}

mod recorded_io {
    use std::{cell::RefCell, io};
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(super) enum Call {
        Open,
        Limit(u64),
        Read,
    }
    #[derive(Default)]
    struct Recording {
        bytes: Vec<u8>,
        fail: u8,
        calls: Vec<Call>,
    }
    thread_local! { static STATE: RefCell<Recording> = RefCell::new(Recording::default()); }
    pub(super) fn reset(bytes: Vec<u8>, fail: u8) {
        STATE.with(|state| {
            *state.borrow_mut() = Recording {
                bytes,
                fail,
                calls: vec![],
            }
        });
    }
    pub(super) fn calls() -> Vec<Call> {
        STATE.with(|state| state.borrow().calls.clone())
    }
    pub(super) struct File {
        limit: u64,
    }
    impl File {
        pub(super) fn open(_path: &std::path::Path) -> io::Result<Self> {
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.calls.push(Call::Open);
                if state.fail == 1 {
                    return Err(io::Error::other("recorded open failure"));
                }
                Ok(Self { limit: u64::MAX })
            })
        }
        pub(super) fn take(mut self, limit: u64) -> Self {
            STATE.with(|state| state.borrow_mut().calls.push(Call::Limit(limit)));
            self.limit = limit;
            self
        }
        pub(super) fn read_to_end(self, bytes: &mut Vec<u8>) -> io::Result<usize> {
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.calls.push(Call::Read);
                if state.fail == 2 {
                    return Err(io::Error::other("recorded read failure"));
                }
                let count = state.bytes.len().min(self.limit as usize);
                bytes.extend_from_slice(&state.bytes[..count]);
                Ok(count)
            })
        }
    }
}

fn normalized(value: Result<CheckEvidence>) -> Value {
    match value {
        Ok((report, pass, results, failed, class)) => json!({"report": report, "pass": pass,
            "results": results, "failed": failed, "class": class}),
        Err(error) => json!({"error": error.to_string()}),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]
    /// #822 extraction: the verbatim old shell and new shell sample one bounded read identically.
    /// There are no clocks or durable writes in this function, hence no write crash points.
    #[test]
    fn evidence_io_schedule_matches_before_extraction(
        pre_existing in any::<bool>(), fail in 0u8..3,
        changed_read in prop::collection::vec(any::<u8>(), 0..64),
        transport_valid in any::<bool>(), alter_bytes in any::<bool>(),
    ) {
        let (job, report) = if pre_existing { fixture("error", "assertion", true, true) }
            else { fixture("passed", "passed", true, false) };
        let original = raw(&report);
        let observed = if alter_bytes { changed_read } else { original.clone() };
        let make_ending = || {
            let mut ending = ending(&original);
            if !transport_valid { ending.result.as_mut().unwrap().verified = false; }
            ending
        };
        recorded_io::reset(observed.clone(), fail);
        let old = normalized(before::run(make_ending(), &job));
        let old_calls = recorded_io::calls();
        recorded_io::reset(observed, fail);
        let new = normalized(evidence_with_read(make_ending(), &job, |log| {
            let mut bytes = Vec::new();
            recorded_io::File::open(&log.path)?.take(log.bytes + 1).read_to_end(&mut bytes)?;
            Ok(bytes)
        }));
        prop_assert_eq!(old, new);
        prop_assert_eq!(old_calls, recorded_io::calls());
    }
}

/// #822/#626: a planned proof still requires its ordinary head to pass in a campaign.
#[test]
fn campaign_planned_proof_requires_positive_ordinary_head() {
    let vector: Value = serde_json::from_str(campaign_case::CASE).unwrap();
    let request = &vector["request"];
    let (mut job, mut report) = fixture("passed", "passed", true, false);
    job.delivery.key = request["effect_key"].as_str().unwrap().into();
    job.delivery.head_sha = Some(request["head"].as_str().unwrap().into());
    job.run.base_sha = serde_json::from_value(request["base"].clone()).unwrap();
    job.run.plan = serde_json::from_value(request["plan"].clone()).unwrap();
    report["effect_key"] = request["effect_key"].clone();
    report["base"] = request["base"].clone();
    report["head"] = request["head"].clone();
    report["results"] = vector["results"].clone();
    // Prove the receipt and campaign are accepted when the ordinary head is positive.
    report["results"][0]["tests"][0]["head"] = json!("passed");
    assert!(parse(&raw(&report), &job).is_some());
    assert!(evaluate(&job, &report).1);
    report["results"][0]["tests"][0]["head"] = json!("error");
    let bytes = raw(&report);
    recorded_io::reset(bytes.clone(), 0);
    let old = normalized(before::run(ending(&bytes), &job));
    assert_eq!(normalized(Ok(evaluate(&job, &report))), old);
    assert!(parse(&bytes, &job).is_none());
    assert_eq!(old["class"], Value::Null);
    assert_eq!(old["pass"], false);
}
