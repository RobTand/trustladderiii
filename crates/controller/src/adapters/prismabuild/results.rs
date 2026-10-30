use super::{Job, checks::Kind};
use crate::{
    Error, Result,
    executor::{Completion, Observation},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read};
use tl3_prismabuild::{ActionKey, Tool};

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
    results: Vec<CheckResult>,
}
#[derive(Deserialize)]
struct CheckResult {
    name: String,
    kind: Kind,
    verdict: Verdict,
    custody: Custody,
    tests: Vec<TestResult>,
}
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Verdict {
    Pass,
    Fail,
    InfrastructureFailure,
}
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Custody {
    InProcess,
}
#[derive(Deserialize)]
struct TestResult {
    test: String,
    base: Outcome,
    head: Outcome,
    pre_existing: bool,
    passed: bool,
}
#[derive(Deserialize)]
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
    let mut report = false;
    let mut pass = false;
    if let Some(evidence) = ending.result {
        if !evidence.verified
            || evidence
                .receipt_sha256
                .as_ref()
                .is_none_or(|s| s.len() != 64)
            || evidence.stdout.bytes > 8 * 1024 * 1024
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
        if let Some(parsed) = parse(&raw, job) {
            report = parsed
                .results
                .iter()
                .all(|r| !matches!(r.verdict, Verdict::InfrastructureFailure));
            pass = report
                && ending.succeeded
                && ending.exit_code == Some(0)
                && parsed
                    .results
                    .iter()
                    .all(|r| matches!(r.verdict, Verdict::Pass));
        }
    } else if matches!(ending.status.as_str(), "executed" | "cache_hit") {
        // A receipt-only cache hit can outlive its queue record. Missing result evidence is unknown.
        return Err(Error::Invalid(
            "successful PrismaBuild ending has no verified result",
        ));
    }
    Ok(Some(Completion::confirmed(vec![Observation::Checks {
        node: job.delivery.node(),
        generation: job.delivery.generation,
        action_key: job.action.to_string(),
        base_sha: job.run.base_sha.clone(),
        sha: job
            .delivery
            .head_sha
            .clone()
            .ok_or(Error::Invalid("missing check head"))?,
        changed_paths: job.changed_paths.clone(),
        report,
        pass,
    }])))
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
        || report.base != job.run.base_sha
        || Some(&report.head) != job.delivery.head_sha.as_ref()
        || report.results.len() != job.run.plan.len()
    {
        return None;
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
            let _ = (&result.base, &result.head, result.pre_existing);
        }
        if matches!(result.verdict, Verdict::Pass) != result.tests.iter().all(|t| t.passed) {
            return None;
        }
    }
    Some(report)
}
