use super::{Job, checks::Kind};
use crate::{
    Error, Result,
    executor::{Completion, Observation},
    findings::{FailedTest, MAX_FAILED_TESTS, MAX_MESSAGE_CHARS, clipped},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read};
use tl3_prismabuild::{ActionKey, Tool};

pub(super) const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
struct Endings {
    schema: String,
    complete: bool,
    endings: Vec<Ending>,
}
#[derive(Deserialize)]
struct Ending {
    action_key: ActionKey,
    status: String,
    succeeded: bool,
    exit_code: Option<i32>,
    result: Option<Evidence>,
    #[serde(default)]
    diagnostics: Diagnostics,
}
#[derive(Default, Deserialize)]
struct Diagnostics {
    returncode: Option<i32>,
    resource_profile: Option<ResourceProfile>,
}
#[derive(Deserialize)]
struct ResourceProfile {
    scope: Option<ResourceScope>,
}
#[derive(Deserialize)]
struct ResourceScope {
    memory_peak_bytes: Option<u64>,
}
#[derive(Deserialize)]
struct Evidence {
    verified: bool,
    receipt_sha256: Option<String>,
    stdout: Log,
}
#[derive(Deserialize)]
struct Log {
    path: std::path::PathBuf,
    bytes: u64,
    sha256: String,
}
#[derive(Deserialize)]
struct Report {
    schema: String,
    effect_key: String,
    base: String,
    head: String,
    #[serde(default)]
    reviewed: Option<String>,
    results: Vec<CheckResult>,
}
#[derive(Deserialize, Serialize)]
struct CheckResult {
    name: String,
    kind: Kind,
    verdict: Verdict,
    custody: Custody,
    tests: Vec<TestResult>,
    /// A forced check echoes its forcing so review can judge it (#393).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    forcing: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proof: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    declared_symptom: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    symptom_evidence: Option<serde_json::Value>,
    /// Complete-unit admission and the unchecked suffix, retained for review (#591).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selection: Option<serde_json::Value>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Verdict {
    Pass,
    Fail,
    InfrastructureFailure,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Custody {
    InProcess,
}
#[derive(Deserialize, Serialize)]
struct TestResult {
    test: String,
    #[serde(rename = "base")]
    _base: Outcome,
    #[serde(rename = "head")]
    _head: Outcome,
    #[serde(rename = "pre_existing")]
    _pre_existing: bool,
    passed: bool,
    #[serde(default)]
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    head_forced: Option<Outcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    phases: Option<PhaseEvidence>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PhaseEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base: Option<PhaseRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    head: Option<PhaseRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    head_forced: Option<PhaseRun>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PhaseRun {
    // A selector expanding to several cases retains its first failing case, or first passing case.
    test: String,
    setup: PhaseFact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    call: Option<PhaseFact>,
    teardown: PhaseFact,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PhaseFact {
    outcome: PhaseOutcome,
    exception: String,
    assertion: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum PhaseOutcome {
    Passed,
    Failed,
    Skipped,
}

impl PhaseEvidence {
    fn valid(&self, requested: &str) -> bool {
        let runs = [
            self.base.as_ref(),
            self.head.as_ref(),
            self.head_forced.as_ref(),
        ];
        runs.iter().any(Option::is_some)
            && runs.into_iter().flatten().all(|run| {
                let selected = run.test == requested
                    || (!requested.contains("::")
                        && run.test.starts_with(&format!("{requested}::")))
                    || (!requested.contains('[')
                        && run.test.starts_with(&format!("{requested}["))
                        && run.test.ends_with(']'));
                selected
                    && run.test.len() <= 4 * 1024 * 1024
                    && (!matches!(run.setup.outcome, PhaseOutcome::Passed) || run.call.is_some())
                    && [
                        ("setup", Some(&run.setup)),
                        ("call", run.call.as_ref()),
                        ("teardown", Some(&run.teardown)),
                    ]
                    .into_iter()
                    .all(|(phase, fact)| {
                        fact.is_none_or(|fact| {
                            fact.exception.chars().count() <= 200
                                && (matches!(fact.outcome, PhaseOutcome::Failed)
                                    || fact.exception.is_empty())
                                && (!fact.assertion
                                    || phase == "call"
                                        && matches!(fact.outcome, PhaseOutcome::Failed))
                        })
                    })
            })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Outcome {
    Passed,
    Assertion,
    AddedName,
    Error,
    Skipped,
    Missing,
    Uncollected,
    NoReport,
}

pub(super) async fn ending(
    tool: &Tool,
    job: &Job,
    withdrawal: bool,
    record: &tl3_prismabuild::RecordRead,
) -> Result<Option<Completion>> {
    let output = tokio::process::Command::new(&tool.program)
        .args(&tool.args)
        .args([
            "--endings",
            "--until",
            "cleaned",
            "--wait-s",
            "0",
            job.action.as_str(),
        ])
        .kill_on_drop(true)
        .output()
        .await?;
    let mut reply: Endings = serde_json::from_slice(&output.stdout)?;
    if reply.schema != "prismabuild.endings.v1" || reply.endings.len() != 1 {
        return Err(Error::Invalid("invalid PrismaBuild endings response"));
    }
    let ending = reply.endings.remove(0);
    if ending.action_key != job.action {
        return Err(Error::Invalid("PrismaBuild ending names another action"));
    }
    if !reply.complete || ending.status == "waiting" {
        return Ok(None);
    }
    if !matches!(
        ending.status.as_str(),
        "executed" | "cache_hit" | "failed" | "withdrawn" | "refused"
    ) {
        return Err(Error::Invalid("unreadable PrismaBuild ending"));
    }
    if withdrawal {
        return Ok(Some(Completion::confirmed(vec![])));
    }
    if !ending.succeeded
        && let tl3_prismabuild::RecordRead::Present(record) = record
        && record.termination_reason.as_deref() == Some("memory_limit_oom")
    {
        let peak = ending
            .diagnostics
            .resource_profile
            .as_ref()
            .and_then(|profile| profile.scope.as_ref())
            .and_then(|scope| scope.memory_peak_bytes);
        let failed = FailedTest {
            test: "PrismaBuild".into(),
            message: format!(
                "memory_limit_oom; action {}; returncode {:?}; memory_peak_bytes {peak:?}",
                job.action,
                ending
                    .exit_code
                    .or(ending.diagnostics.returncode)
                    .or(record.returncode)
            ),
        };
        return Ok(Some(completion(job, false, false, vec![], vec![failed])?));
    }
    let (report, pass, results, failed_tests, failure) = match evidence(ending, job) {
        Ok(verdict) => verdict,
        Err(error @ Error::Io(_)) => return Err(error),
        Err(error) => {
            eprintln!("PrismaBuild result {}: {error}", job.action);
            (false, false, vec![], vec![], None)
        }
    };
    let mut completion = completion(job, report, pass, results, failed_tests)?;
    completion.failure_class = failure.map(Into::into);
    Ok(Some(completion))
}

/// The failure class when TL3's trusted harness rejected its own check input:
/// TL3-side under Q51, not a PrismaBuild failure.
pub(crate) const HARNESS: &str = "harness";

/// The harness's `SELECTED_CHECK`: existing tests that import the change (#359).
const SELECTED_CHECK: &str = "Existing tests that import the change";

type CheckEvidence = (
    bool,
    bool,
    Vec<serde_json::Value>,
    Vec<FailedTest>,
    Option<&'static str>,
);

fn evidence(ending: Ending, job: &Job) -> Result<CheckEvidence> {
    let successful = matches!(ending.status.as_str(), "executed" | "cache_hit")
        && ending.succeeded
        && ending.exit_code == Some(0);
    let mut report = false;
    let mut pass = false;
    let mut results = vec![];
    let mut failed_tests = Vec::new();
    let mut failure = None;
    if let Some(evidence) = ending.result {
        if !evidence.verified
            || (successful
                && evidence.receipt_sha256.as_ref().is_none_or(|s| {
                    s.len() != 64
                        || !s
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                }))
            || evidence.stdout.bytes > MAX_RESULT_BYTES as u64
        {
            return Err(Error::Invalid(
                "unverified or oversized PrismaBuild check result",
            ));
        }
        let mut raw = Vec::new();
        File::open(&evidence.stdout.path)?
            .take(evidence.stdout.bytes + 1)
            .read_to_end(&mut raw)?;
        if raw.len() as u64 != evidence.stdout.bytes
            || format!("{:x}", Sha256::digest(&raw)) != evidence.stdout.sha256
        {
            return Err(Error::Invalid("PrismaBuild result log changed"));
        }
        if let Some(mut parsed) = parse(&raw, job) {
            for test in parsed
                .results
                .iter_mut()
                .flat_map(|result| &mut result.tests)
            {
                test.message = clipped(&test.message, MAX_MESSAGE_CHARS);
            }
            failed_tests = parsed
                .results
                .iter()
                .flat_map(|result| &result.tests)
                .filter(|test| !test.passed)
                .take(MAX_FAILED_TESTS)
                .map(|test| FailedTest {
                    test: clipped(&test.test, 4096),
                    message: if test.message.trim().is_empty() {
                        "The harness rejected this named test; no failure message was supplied."
                            .into()
                    } else {
                        clipped(&test.message, MAX_MESSAGE_CHARS)
                    },
                })
                .collect();
            report = parsed
                .results
                .iter()
                .all(|r| !matches!(r.verdict, Verdict::InfrastructureFailure));
            pass = report
                && successful
                && parsed
                    .results
                    .iter()
                    .all(|r| matches!(r.verdict, Verdict::Pass));
            results = parsed
                .results
                .iter()
                .map(serde_json::to_value)
                .collect::<std::result::Result<_, _>>()?;
        } else if harness_error(&raw, job) {
            failure = Some(HARNESS);
        }
    } else if matches!(ending.status.as_str(), "executed" | "cache_hit") {
        // A receipt-only cache hit can outlive its queue record. Missing result evidence is unknown.
        return Err(Error::Invalid(
            "successful PrismaBuild ending has no verified result",
        ));
    }
    Ok((report, pass, results, failed_tests, failure))
}

fn harness_error(raw: &[u8], job: &Job) -> bool {
    #[derive(Deserialize)]
    struct HarnessError {
        schema: String,
        effect_key: String,
    }
    let Ok(text) = std::str::from_utf8(raw) else {
        return false;
    };
    let errors: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("TL3_CHECKS_ERROR "))
        .collect();
    errors.len() == 1
        && serde_json::from_str::<HarnessError>(errors[0]).is_ok_and(|error| {
            error.schema == "tl3.checks.error.v1" && error.effect_key == job.delivery.key
        })
}

fn completion(
    job: &Job,
    report: bool,
    pass: bool,
    results: Vec<serde_json::Value>,
    failed_tests: Vec<FailedTest>,
) -> Result<Completion> {
    let head = job
        .delivery
        .head_sha
        .clone()
        .ok_or(Error::Invalid("missing check head"))?;
    let mut observations = vec![];
    if job.delivery.checks.as_ref() != Some(&job.run) {
        // Staging moved the plan to the base the head descends from; keep it.
        observations.push(Observation::CheckPlan {
            node: job.delivery.node(),
            sha: head.clone(),
            run: job.run.clone(),
        });
    }
    observations.push(Observation::Checks {
        node: job.delivery.node(),
        generation: job.delivery.generation,
        action_key: job.action.to_string(),
        base_sha: job.run.base_sha.as_str().to_owned(),
        sha: head,
        changed_paths: job.changed_paths.clone(),
        failed_tests,
        report,
        pass,
        results,
    });
    Ok(Completion::confirmed(observations))
}

fn parse(raw: &[u8], job: &Job) -> Option<Report> {
    let text = std::str::from_utf8(raw).ok()?;
    let reports: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("TL3_CHECKS "))
        .collect();
    if reports.len() != 1 {
        return None;
    }
    let report: Report = serde_json::from_str(reports[0]).ok()?;
    if report.schema != "tl3.checks.v1"
        || report.effect_key != job.delivery.key
        || report.base != job.run.base_sha.as_str()
        || Some(&report.head) != job.delivery.head_sha.as_ref()
        || report.reviewed.as_deref()
            != job
                .run
                .reviewed_sha
                .as_ref()
                .map(tl3_worktrees::CommitId::as_str)
    {
        return None;
    }
    if report
        .results
        .iter()
        .flat_map(|result| &result.tests)
        .any(|test| {
            test.phases
                .as_ref()
                .is_some_and(|phases| !phases.valid(&test.test))
        })
    {
        return None;
    }
    let mut plan = job.run.plan.clone();
    if !job.always_tests.is_empty() {
        plan.push(super::Check {
            name: "Repository always tests".into(),
            kind: Kind::Regression,
            tests: job.always_tests.clone(),
            forcing: None,
            proof: None,
            declared_symptom: None,
        });
    }
    // Only a job staged with selection may carry one trailing selected check (#359).
    let extra = report.results.len().checked_sub(plan.len())?;
    if extra > usize::from(job.selected_tests.is_some()) {
        return None;
    }
    if let Some(selected) = report.results.get(plan.len()) {
        let Custody::InProcess = selected.custody;
        if selected.name != SELECTED_CHECK
            || selected.kind != Kind::Regression
            || selected.tests.is_empty()
            || selected.tests.len() > job.selected_tests.unwrap_or(0) as usize
            || matches!(selected.verdict, Verdict::Pass) != selected.tests.iter().all(|t| t.passed)
        {
            return None;
        }
    }
    for (result, check) in report.results.iter().zip(&plan) {
        if result.name != check.name
            || result.kind != check.kind
            || result.tests.len() != check.tests.len()
        {
            return None;
        }
        let Custody::InProcess = result.custody;
        if result.declared_symptom != check.declared_symptom {
            return None;
        }
        if let Some(declaration) = &check.declared_symptom {
            if !symptom_identity(result, declaration, check, job) {
                return None;
            }
        } else if result.symptom_evidence.is_some() {
            return None;
        }
        for (result, test) in result.tests.iter().zip(&check.tests) {
            if &result.test != test {
                return None;
            }
        }
        if matches!(result.verdict, Verdict::Pass) != result.tests.iter().all(|t| t.passed) {
            return None;
        }
        if let Some(identity) = &result.proof {
            let hash = |v: &serde_json::Value| {
                v.as_str().is_some_and(|s| {
                    s.len() == 64
                        && s.bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
            };
            if !matches!(check.kind, Kind::Demonstrating | Kind::Reproducing)
                || identity["rule"] != "subject-observer-v1"
                || identity["declaration"] != check.proof.clone().unwrap_or(serde_json::Value::Null)
                || ![
                    "source_base_tree_sha256",
                    "base_tree_sha256",
                    "head_tree_sha256",
                ]
                .iter()
                .all(|key| hash(&identity[key]))
                || !matches!(
                    identity["mode"].as_str(),
                    Some("head-observer" | "pytest-outcome")
                )
                || identity["observer_sha256"]
                    .as_object()
                    .is_none_or(|observers| {
                        observers.len() != check.tests.len()
                            || check
                                .tests
                                .iter()
                                .any(|test| !observers.get(test).is_some_and(hash))
                    })
                || identity["subjects"].as_object().is_none_or(|subjects| {
                    subjects.values().any(|subject| {
                        ["base", "head"]
                            .iter()
                            .any(|key| !subject[key].is_null() && !hash(&subject[key]))
                    })
                })
                || identity["support_sha256"]
                    .as_object()
                    .is_none_or(|support| support.values().any(|value| !hash(value)))
            {
                return None;
            }
            if let Some(declaration) = &check.proof {
                let subjects = declaration["subjects"].as_array()?;
                let support = declaration["observer_support"].as_array()?;
                if subjects.len() > 16
                    || support.len() > 16
                    || declaration["red_message"]
                        .as_str()
                        .is_none_or(str::is_empty)
                    || (!subjects.is_empty()
                        && identity["subjects"].as_object()?.len() != subjects.len())
                    || subjects.iter().any(|unit| {
                        let value = &identity["subjects"][unit.as_str().unwrap_or("")];
                        value.is_null() || value["base"] == value["head"]
                    })
                    || identity["support_sha256"].as_object()?.len() != support.len()
                    || support
                        .iter()
                        .any(|unit| !hash(&identity["support_sha256"][unit.as_str().unwrap_or("")]))
                    || result.tests.iter().any(|test| {
                        test.passed
                            && ((check.declared_symptom.is_none()
                                && !matches!(test._base, Outcome::Assertion | Outcome::AddedName))
                                || !matches!(test._head, Outcome::Passed))
                    })
                {
                    return None;
                }
            }
        } else if check.proof.is_some() && result.tests.iter().any(|test| test.passed) {
            return None;
        }
        if check.proof.is_none()
            && matches!(check.kind, Kind::Demonstrating | Kind::Reproducing)
            && check.declared_symptom.is_none()
            && result
                .tests
                .iter()
                .any(|test| test.passed && matches!(test._base, Outcome::Assertion))
        {
            return None;
        }
        // A forced check passes only on evidence from its forced head run (#393).
        if result.forcing.is_some() != check.forcing.is_some()
            || (check.forcing.is_some()
                && result
                    .tests
                    .iter()
                    .any(|t| t.passed && !matches!(t.head_forced, Some(Outcome::Passed))))
        {
            return None;
        }
    }
    Some(report)
}

fn symptom_identity(
    result: &CheckResult,
    declaration: &serde_json::Value,
    check: &super::Check,
    job: &Job,
) -> bool {
    let Some(evidence) = &result.symptom_evidence else {
        // Refusal before execution carries no positive proof.
        return !matches!(result.verdict, Verdict::Pass);
    };
    if evidence["base"] != job.run.base_sha.as_str()
        || evidence["head"].as_str() != job.delivery.head_sha.as_deref()
        || evidence["declaration"] != *declaration
        || evidence["forcing"] != check.forcing.clone().unwrap_or(serde_json::Value::Null)
        || check.kind != Kind::Demonstrating
        || check.tests.len() != 1
        || declaration["node"].as_str() != Some(check.tests[0].as_str())
    {
        return false;
    }
    let Some(repeat @ 1..=10) = declaration["repeat"].as_u64() else {
        return false;
    };
    let Some(runs) = evidence["runs"].as_array() else {
        return false;
    };
    let mut red = 0;
    let mut green = true;
    let sides = if check.forcing.is_some() {
        &["base", "head", "ordinary-head"][..]
    } else {
        &["base", "head"][..]
    };
    for side in sides {
        let selected: Vec<_> = runs.iter().filter(|run| run["side"] == *side).collect();
        if selected.len() != repeat as usize {
            return !matches!(result.verdict, Verdict::Pass);
        }
        for (index, run) in selected.iter().enumerate() {
            let Some(records) = run["records"].as_array() else {
                return false;
            };
            let Ok(encoded) = serde_json::to_vec(records) else {
                return false;
            };
            if run["run"].as_u64() != Some(index as u64 + 1)
                || run["sha256"] != format!("{:x}", Sha256::digest(encoded))
                || records.is_empty()
            {
                return false;
            }
            match run["outcome"].as_str() {
                Some("symptom") if *side == "base" => red += 1,
                Some("passed") => (),
                _ => green = false,
            }
        }
    }
    runs.len() == repeat as usize * sides.len()
        && (!matches!(result.verdict, Verdict::Pass) || (red > 0 && green))
}

/// Tests that failed a selected check in this round or an earlier one: once a maker edits
/// such a test it is no longer selected, so review is told about it.
pub(crate) fn selected_failures<'a>(
    previous: impl IntoIterator<Item = &'a serde_json::Value>,
    results: &[serde_json::Value],
) -> Vec<String> {
    let mut failures = std::collections::BTreeSet::new();
    for report in previous {
        failures.extend(
            report["selected_failures"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|test| test.as_str().map(str::to_owned)),
        );
    }
    for result in results.iter().filter(|r| r["name"] == SELECTED_CHECK) {
        failures.extend(
            result["tests"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|test| test["passed"] == false)
                .filter_map(|test| test["test"].as_str().map(str::to_owned)),
        );
    }
    failures.into_iter().collect()
}

#[cfg(test)]
mod selected_failure_tests {
    use serde_json::json;

    #[test]
    fn saturation_receipt_survives_controller_report_serialization() {
        let receipt = json!({"limit": 2, "charged_cases": 1,
            "head_units": [["tests/test_a.py"]], "skipped_units": [["tests/test_b.py"]],
            "skipped_checked": false,
            "saturation": {"unit": ["tests/test_b.py"], "cases": 2, "remaining": 1}});
        let result: super::CheckResult = serde_json::from_value(json!({
            "name": super::SELECTED_CHECK, "kind": "regression", "verdict": "pass",
            "custody": "in-process", "tests": [], "selection": receipt
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap()["selection"],
            receipt,
            "review must see that the skipped suffix was not checked"
        );
    }

    #[test]
    fn selected_failures_accumulate_across_rounds() {
        let earlier = json!({"base_sha": "b", "results": [], "selected_failures": ["tests/test_a.py::test_a"]});
        let results = [
            json!({"name": "planned", "kind": "regression", "tests": [
                {"test": "tests/test_p.py::test_p", "passed": false}]}),
            json!({"name": super::SELECTED_CHECK, "kind": "regression", "tests": [
                {"test": "tests/test_b.py::test_b", "passed": false},
                {"test": "tests/test_c.py::test_c", "passed": true}]}),
        ];
        assert_eq!(
            super::selected_failures([&earlier], &results),
            ["tests/test_a.py::test_a", "tests/test_b.py::test_b"],
            "keep earlier failures and add this round's selected failures, not planned ones"
        );
    }
}
