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

pub(super) async fn ending(tool: &Tool, job: &Job, withdrawal: bool) -> Result<Option<Completion>> {
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
    // Only a job staged with selection may carry one trailing selected check (#359).
    let extra = report.results.len().checked_sub(job.run.plan.len())?;
    if extra > usize::from(job.selected_tests.is_some()) {
        return None;
    }
    if let Some(selected) = report.results.get(job.run.plan.len()) {
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
    for (result, check) in report.results.iter().zip(&job.run.plan) {
        if result.name != check.name
            || result.kind != check.kind
            || result.tests.len() != check.tests.len()
        {
            return None;
        }
        let Custody::InProcess = result.custody;
        for (result, test) in result.tests.iter().zip(&check.tests) {
            if &result.test != test {
                return None;
            }
        }
        if matches!(result.verdict, Verdict::Pass) != result.tests.iter().all(|t| t.passed) {
            return None;
        }
    }
    Some(report)
}

/// Red scaffold for #409's review: earlier selected-check failures are not carried yet.
pub(crate) fn selected_failures<'a>(
    _previous: impl IntoIterator<Item = &'a serde_json::Value>,
    _results: &[serde_json::Value],
) -> Vec<String> {
    vec![]
}

#[cfg(test)]
mod selected_failure_tests {
    use serde_json::json;

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
