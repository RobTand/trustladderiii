use super::super::Check;
use super::*;
use proptest::prelude::*;
use serde_json::{Value, json};

#[path = "before.rs"]
mod before;
#[path = "evidence-current.rs"]
mod current;

mod campaign_case;
mod producer_cases;
mod tessera_case;

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
        let (mut old, _) = recorded_io::run(
            vec![],
            vec![recorded_io::Step::Bytes(bytes.clone())],
            || normalized_before(before::run(ending(&bytes), &job)),
        );
        // #849 intentionally changes only the class of the producer's no-report case.
        let expected_class = if selected {
            Value::Null
        } else {
            json!(HARNESS)
        };
        assert_eq!(old["class"], Value::Null);
        old["class"] = expected_class.clone();
        let new = normalized(Ok(evaluate(&job, &report)));
        assert_eq!(new, old, "producer override selected={selected}");
        assert_eq!(new["class"], expected_class);
        assert_eq!(new["pass"], false);
        assert!(!new["results"].as_array().unwrap().is_empty());
    }
}

#[test]
fn disagreement_uses_the_confirmed_harness_class() {
    let (job, report) = fixture("skipped", "passed", true, false);
    assert!(
        parse(&raw(&report), &job).unwrap().is_some(),
        "shape/identity validation must not swallow a judge disagreement"
    );
    let (report, pass, results, failed, class, diagnostic) = evaluate(&job, &report);
    assert!(!report && !pass && results.is_empty() && failed.is_empty());
    assert_eq!(class, Some(HARNESS));
    assert!(diagnostic.is_none());
    assert_eq!(JUDGE_MISMATCH_CLASS, Some(HARNESS));
}

async fn fixture_completion(job: &Job, bytes: &[u8]) -> Completion {
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("stdout");
    std::fs::write(&log, bytes).unwrap();
    let response = json!({"schema":"prismabuild.endings.v1","complete":true,
        "endings":[{"action_key":job.action,"status":"executed","succeeded":true,"exit_code":0,
            "result":{"verified":true,"receipt_sha256":"c".repeat(64),"stdout":{
                "path":log,"bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(bytes))}}}]});
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
    super::ending(&tool, job, false, &tl3_prismabuild::RecordRead::Absent)
        .await
        .unwrap()
        .unwrap()
}

async fn mismatch_completion(base: &str, python_passed: bool) {
    let (mut job, report) = fixture(base, "passed", python_passed, false);
    job.delivery.checks = Some(job.run.clone());
    let completion = fixture_completion(&job, &raw(&report)).await;
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
    let (reported, passed, results, failed, class, diagnostic) = evaluate(&job, &report);
    assert!(diagnostic.is_none());
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
    assert!(parse(&raw(&report), &job).unwrap().is_none());
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

pub(super) mod recorded_io;

fn normalized_before(value: Result<before::CheckEvidence>) -> Value {
    normalized(
        value.map(|(report, pass, results, failed, class)| {
            (report, pass, results, failed, class, None)
        }),
    )
}

fn normalized(value: Result<CheckEvidence>) -> Value {
    match value {
        Ok((report, pass, results, failed, class, diagnostic)) => {
            json!({"report": report, "pass": pass,
            "results": results, "failed": failed, "class": class, "diagnostic": diagnostic})
        }
        Err(error) => json!({"error": error.to_string()}),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    /// #822 extraction: execute both production shells against per-call File schedules.
    /// No clocks or durable writes occur here, so there is no persisted crash state.
    #[test]
    fn evidence_io_schedule_matches_before_extraction(
        shape in 0u8..6, opens in prop::collection::vec(any::<bool>(), 0..5),
        reads in prop::collection::vec((0u8..5, prop::collection::vec(any::<u8>(), 0..96)), 0..8),
        transport_valid in any::<bool>(), successful in any::<bool>(),
    ) {
        let (job, mut report) = match shape {
            0 => fixture("error", "assertion", true, true),
            1 => fixture("passed", "passed", true, false),
            2 => fixture("no-report", "passed", false, false),
            3 => fixture("passed", "error", false, false),
            4 => fixture("added-name", "passed", true, false),
            _ => fixture("skipped", "passed", false, false),
        };
        if shape == 1 {
            report["results"][0]["tests"][0]["phases"] = json!({"head": phases("passed", false)});
        }
        let original = raw(&report);
        let schedule: Vec<_> = reads.into_iter().map(|(kind, bytes)| match kind {
            0 | 1 => recorded_io::Step::Bytes(original.clone()),
            2 => recorded_io::Step::Bytes(bytes),
            3 => recorded_io::Step::Bytes(vec![]),
            _ => recorded_io::Step::Fail,
        }).collect();
        let make_ending = || {
            let mut ending = ending(&original);
            if !transport_valid { ending.result.as_mut().unwrap().verified = false; }
            if !successful {
                ending.status = "failed".into();
                ending.succeeded = false;
                ending.exit_code = Some(1);
                ending.result.as_mut().unwrap().receipt_sha256 = None;
            }
            ending
        };
        let (mut old, old_calls) = recorded_io::run(opens.clone(), schedule.clone(),
            || normalized_before(before::run(make_ending(), &job)));
        let (pinned, pinned_calls) = recorded_io::run(opens.clone(), schedule.clone(),
            || normalized_before(current::run(make_ending(), &job)));
        let (new, new_calls) = recorded_io::run(opens, schedule,
            || normalized(evidence(make_ending(), &job)));
        prop_assert_eq!(&pinned_calls, &new_calls);
        prop_assert_eq!(&pinned, &new);
        prop_assert_eq!(old_calls, new_calls);
        // Q51/#849 changes the class only for successful PB with parsed no-report.
        // All reads, findings, results and other outcomes still match the frozen shell.
        if successful && shape == 2 && old["results"].as_array().is_some_and(|r| !r.is_empty()) {
            prop_assert_eq!(&old["class"], &Value::Null);
            old["class"] = json!(HARNESS);
        }
        prop_assert_eq!(old, new);
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
    assert!(parse(&raw(&report), &job).unwrap().is_some());
    assert!(evaluate(&job, &report).1);
    report["results"][0]["tests"][0]["head"] = json!("error");
    let bytes = raw(&report);
    let (old, _) = recorded_io::run(
        vec![],
        vec![recorded_io::Step::Bytes(bytes.clone())],
        || normalized_before(before::run(ending(&bytes), &job)),
    );
    assert_eq!(normalized(Ok(evaluate(&job, &report))), old);
    assert!(parse(&bytes, &job).unwrap().is_none());
    assert_eq!(old["class"], Value::Null);
    assert_eq!(old["pass"], false);
}

fn shell_invariant_counterexample(selected: bool, forced: bool) {
    let (mut job, mut report) = fixture("passed", "passed", true, false);
    let mut appended = report["results"][0].clone();
    if selected {
        job.selected_tests = Some(1);
        appended["name"] = json!(SELECTED_CHECK);
    } else {
        job.always_tests = vec!["tests/test_x.py::test_x".into()];
        appended["name"] = json!("Repository always tests");
    }
    report["results"].as_array_mut().unwrap().push(appended);
    assert!(
        evaluate(&job, &report).1,
        "the unaltered shell report must pass"
    );
    if forced {
        report["results"][1]["tests"][0]["head_forced"] = json!("passed");
    } else {
        report["results"][1]["tests"][0]["pre_existing"] = json!(true);
    }
    assert!(
        parse(&raw(&report), &job).unwrap().is_some(),
        "the counterexample is shape-valid"
    );
    let (reported, passed, results, failed, class, diagnostic) = evaluate(&job, &report);
    assert!(diagnostic.is_none());
    assert!(!reported && !passed && results.is_empty() && failed.is_empty());
    assert_eq!(class, Some(HARNESS));
}

/// #822: stale_result leaves the judge's pre_existing flag intact.
#[test]
fn shell_invariants_always_pre_existing() {
    shell_invariant_counterexample(false, false);
}

/// #822: selected_head leaves the judge's pre_existing flag intact.
#[test]
fn shell_invariants_selected_pre_existing() {
    shell_invariant_counterexample(true, false);
}

/// #822/#393: an always check cannot invent a forced-head observation.
#[test]
fn shell_invariants_always_forced_head() {
    shell_invariant_counterexample(false, true);
}

/// #822/#393: a selected group cannot invent a forced-head observation.
#[test]
fn shell_invariants_selected_forced_head() {
    shell_invariant_counterexample(true, true);
}

fn tessera() -> (Job, Value) {
    let report: Value = serde_json::from_str(tessera_case::REPORT).unwrap();
    let (mut job, _) = fixture("passed", "passed", true, false);
    job.delivery.key = report["effect_key"].as_str().unwrap().into();
    job.delivery.head_sha = Some(report["head"].as_str().unwrap().into());
    job.run.base_sha = serde_json::from_value(report["base"].clone()).unwrap();
    job.run.plan = report["results"].as_array().unwrap().iter().map(|result| {
        let mut check = json!({"name":result["name"], "kind":result["kind"],
            "tests":result["tests"].as_array().unwrap().iter().map(|test| test["test"].clone()).collect::<Vec<_>>()});
        if let Some(proof) = result.get("proof") {
            check["proof"] = proof["declaration"].clone();
        }
        serde_json::from_value(check).unwrap()
    }).collect();
    job.delivery.checks = Some(job.run.clone());
    (job, report)
}

/// #848/live probe: preserve Tessera #1194's complete failed RED as a work failure.
#[test]
fn tessera_1194_mismatch_keeps_the_real_failed_check() {
    let (job, report) = tessera();
    let (reported, passed, results, failed, class) = evaluate(&job, &report);
    let completion = completion(&job, reported, passed, results.clone(), failed.clone()).unwrap();
    println!(
        "TESSERA_COMPLETION {}",
        serde_json::to_string(&completion).unwrap()
    );
    assert!(reported && !passed);
    assert_eq!(class, None);
    assert_eq!(
        results
            .iter()
            .map(|r| r["verdict"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["fail", "pass", "pass", "pass"]
    );
    assert_eq!(
        results
            .iter()
            .map(|r| r["tests"].as_array().unwrap().len())
            .collect::<Vec<_>>(),
        [1, 8, 11, 1]
    );
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].test, report["results"][0]["tests"][0]["test"]);
    assert_eq!(
        failed[0].message,
        report["results"][0]["tests"][0]["message"]
    );
}

/// #848: a mismatch must never satisfy the declared RED requirement.
#[test]
fn tessera_mismatch_cannot_be_claimed_as_a_pass() {
    let (job, mut report) = tessera();
    report["results"][0]["verdict"] = json!("pass");
    report["results"][0]["tests"][0]["passed"] = json!(true);
    assert_eq!(evaluate(&job, &report).4, Some(HARNESS));
}

/// #848/Q51: a complete but undecodable report is a diagnosed TL3 defect.
#[test]
fn schema_decode_failure_is_a_bounded_harness_diagnostic() {
    let (job, report) = fixture("passed", "passed", true, false);
    let mut cases = Vec::new();
    let mut unknown = report.clone();
    unknown["results"][0]["tests"][0]["head"] = json!("future-outcome");
    cases.push(unknown);
    let mut missing = report.clone();
    missing["results"][0]
        .as_object_mut()
        .unwrap()
        .remove("verdict");
    cases.push(missing);
    let mut version = report.clone();
    version["schema"] = json!("tl3.checks.future");
    cases.push(version);
    let mut long = report.clone();
    long["results"][0]["tests"][0]["head"] = json!("x".repeat(4000));
    cases.push(long);
    for report in cases {
        let (reported, passed, results, failed, class) = evaluate(&job, &report);
        assert_eq!(
            class,
            Some(HARNESS),
            "schema error was stored as generic no-report"
        );
        assert!(!reported && !passed && results.is_empty());
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].test, "TL3_CHECKS");
        assert_eq!(
            failed[0].message,
            "report schema decode failed: data at line 0 column 0"
        );
        assert!(failed[0].message.chars().count() <= MAX_MESSAGE_CHARS);
    }
}

/// #848: absence and truncated output retain their existing unknown/no-report path.
#[test]
fn missing_and_truncated_reports_remain_unclassified() {
    let (job, _) = fixture("passed", "passed", true, false);
    for raw in [
        b"".as_slice(),
        b"no report\n",
        b"TL3_CHECKS {\"schema\":",
        b"TL3_CHECKS ",
        b"TL3_CHECKS {\"schema\":\"future\"",
        b"\xff",
    ] {
        let result = evidence_with_read(ending(raw), &job, |_| Ok(raw.to_vec())).unwrap();
        assert!(!result.0 && !result.1 && result.2.is_empty() && result.3.is_empty());
        assert_eq!(result.4, None);
    }
}

/// #848: enumerate the actual Python emitter, so a new member cannot silently disappear.
#[test]
fn python_report_string_enums_deserialize_in_rust() {
    use crate::contracts::{checks::CheckKind, report};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../harness/src");
    let output = std::process::Command::new("python3")
        .args([
            "-B",
            "-c",
            r#"
import json
from tl3_harness.model import Kind, Outcome, Custody, Verdict
from tl3_harness.red_message import RedMessageMatch
print(json.dumps({enum.__name__: [member.value for member in enum.__members__.values()]
                  for enum in (Kind, Outcome, Custody, Verdict, RedMessageMatch)}))
"#,
        ])
        .env("PYTHONPATH", root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Python emitter enumeration failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let enums: std::collections::BTreeMap<String, Vec<String>> =
        serde_json::from_slice(&output.stdout).unwrap();
    for (name, values) in enums {
        for value in values {
            let value_json = json!(value);
            let parsed = match name.as_str() {
                "Kind" => serde_json::from_value::<CheckKind>(value_json).map(|_| ()),
                "Outcome" => serde_json::from_value::<report::Outcome>(value_json).map(|_| ()),
                "Custody" => serde_json::from_value::<report::Custody>(value_json).map(|_| ()),
                "Verdict" => serde_json::from_value::<report::Verdict>(value_json).map(|_| ()),
                "RedMessageMatch" => {
                    serde_json::from_value::<report::RedMessageMatch>(value_json).map(|_| ())
                }
                _ => panic!("unhandled producer enum {name}"),
            };
            assert!(
                parsed.is_ok(),
                "Python {name} member {value:?} is missing from Rust: {parsed:?}"
            );
        }
    }
}

/// #848 review: a schema diagnostic must not persist rejected report values.
#[tokio::test]
async fn review_848_decode_diagnostic_never_persists_payload_values() {
    const MARKER: &str = "t848-synthetic-secret-9c65f921e04a";
    let (mut job, mut report) = fixture("passed", "passed", true, false);
    job.delivery.checks = Some(job.run.clone());
    report["results"][0]["tests"][0]["head"] = json!(MARKER);
    let completion = fixture_completion(&job, &raw(&report)).await;
    let persisted = serde_json::to_string(&completion).unwrap();
    assert!(
        !persisted.contains(MARKER),
        "report payload value leaked into persisted completion"
    );
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
        panic!("expected exactly the persisted Checks observation")
    };
    assert!(!report && !pass && results.is_empty());
    assert_eq!(failed_tests.len(), 1);
    assert!(
        failed_tests[0]
            .message
            .starts_with("report schema decode failed: data at line ")
    );
}

/// #848 review: JSON framing failures keep the base's entire completion, schema failures do not.
#[tokio::test]
async fn review_848_syntax_failure_keeps_the_base_completion() {
    let (mut job, mut report) = fixture("passed", "passed", true, false);
    job.delivery.checks = Some(job.run.clone());
    for bytes in [
        br#"TL3_CHECKS {"schema":}"#.as_slice(),
        br#"TL3_CHECKS {"schema":"tl3.checks.v1",}"#,
        b"TL3_CHECKS [1,,2]",
    ] {
        let ((reported, passed, results, failed, class), _) = recorded_io::run(
            vec![],
            vec![recorded_io::Step::Bytes(bytes.to_vec())],
            || before::run(ending(bytes), &job).unwrap(),
        );
        let mut base = completion(&job, reported, passed, results, failed).unwrap();
        base.failure_class = class.map(Into::into);
        let current = fixture_completion(&job, bytes).await;
        assert_eq!(
            serde_json::to_value(current).unwrap(),
            serde_json::to_value(base).unwrap(),
            "JSON syntax failure must keep its base-identical completion"
        );
    }
    // Valid JSON reaches typed schema decoding and still gets the harness class.
    report["results"][0]
        .as_object_mut()
        .unwrap()
        .remove("verdict");
    let schema_failure = fixture_completion(&job, &raw(&report)).await;
    assert_eq!(schema_failure.failure_class.as_deref(), Some(HARNESS));
}

/// #849 differential pin: preserve the frozen reference and pin today's evidence shell.
#[test]
fn current_evidence_shell_matches_production() {
    let pin = include_str!("evidence-current.rs");
    let body = pin
        .split("// BEGIN BODY\n")
        .nth(1)
        .unwrap()
        .split("\n// END BODY")
        .next()
        .unwrap();
    assert!(include_str!("../../results.rs").contains(body));
}
