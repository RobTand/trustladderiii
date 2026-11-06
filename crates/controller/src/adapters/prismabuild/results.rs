use super::{Job, checks::Kind};
use crate::{
    Error, Result,
    contracts::report::{
        CheckReport as Report, CheckResult, Custody, Outcome, PhaseEvidence, PhaseOutcome,
        RedMessageMatch, RunOutcome, SymptomSide, Verdict,
    },
    executor::{Completion, Observation},
    findings::{FailedTest, MAX_FAILED_TESTS, MAX_MESSAGE_CHARS, clipped},
};
#[cfg(test)]
use judge_tests::recorded_io::File;
use serde::Deserialize;
use sha2::{Digest, Sha256};
#[cfg(not(test))]
use std::fs::File;
use std::io::Read;
use tl3_harness_core::{judge as core_judge, phase as core_phase};
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
// The typed report states each field's shape and bound. The rules below relate fields to
// each other and to the staged job.
fn phases_valid(phases: &PhaseEvidence, requested: &str) -> bool {
    [phases.base(), phases.head(), phases.head_forced()]
        .into_iter()
        .flatten()
        .all(|run| {
            let run_test = run.test.as_str();
            let selected = run_test == requested
                || (!requested.contains("::") && run_test.starts_with(&format!("{requested}::")))
                || (!requested.contains('[')
                    && run_test.starts_with(&format!("{requested}["))
                    && run_test.ends_with(']'));
            selected
                && run_test.len() <= 4 * 1024 * 1024
                && (!matches!(run.setup.outcome, PhaseOutcome::Passed) || run.call.is_some())
                && [
                    ("setup", Some(&run.setup)),
                    ("call", run.call.as_ref()),
                    ("teardown", Some(&run.teardown)),
                ]
                .into_iter()
                .all(|(phase, fact)| {
                    fact.is_none_or(|fact| {
                        (matches!(fact.outcome, PhaseOutcome::Failed)
                            || fact.exception.as_str().is_empty())
                            && (!fact.assertion
                                || phase == "call" && matches!(fact.outcome, PhaseOutcome::Failed))
                    })
                })
        })
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

/// TL3's trusted harness failures, including parsed test-run infrastructure outcomes.
/// TL3-side under Q51, not a PrismaBuild failure.
pub(crate) const HARNESS: &str = "harness";

/// Q51/#822: disagreement between TL3's producer and judge is a TL3-side defect.
const JUDGE_MISMATCH_CLASS: Option<&str> = Some(HARNESS);

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
    evidence_with_read(ending, job, |log| {
        let mut raw = Vec::new();
        File::open(&log.path)?
            .take(log.bytes + 1)
            .read_to_end(&mut raw)?;
        Ok(raw)
    })
}

fn evidence_with_read(
    ending: Ending,
    job: &Job,
    read: impl FnOnce(&Log) -> Result<Vec<u8>>,
) -> Result<CheckEvidence> {
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
        let raw = read(&evidence.stdout)?;
        if raw.len() as u64 != evidence.stdout.bytes
            || format!("{:x}", Sha256::digest(&raw)) != evidence.stdout.sha256
        {
            return Err(Error::Invalid("PrismaBuild result log changed"));
        }
        // The typed report clips each message to MAX_MESSAGE_CHARS as it parses.
        let parsed = match parse(&raw, job) {
            Ok(parsed) => parsed,
            Err(error) => {
                // #848/Q51: a complete report that TL3 cannot decode is a harness defect.
                // Serde's display text can quote report values; persist only a safe category.
                let category = match error.classify() {
                    serde_json::error::Category::Io => "io",
                    serde_json::error::Category::Syntax => "syntax",
                    serde_json::error::Category::Data => "data",
                    serde_json::error::Category::Eof => "eof",
                };
                let diagnostic = FailedTest {
                    test: "TL3_CHECKS".into(),
                    message: format!(
                        "report schema decode failed: {category} at line {} column {}",
                        error.line(),
                        error.column(),
                    ),
                };
                return Ok((false, false, vec![], vec![diagnostic], Some(HARNESS)));
            }
        };
        if let Some(parsed) = parsed {
            if !rejudged(&parsed, job) {
                return Ok((false, false, vec![], vec![], JUDGE_MISMATCH_CLASS));
            }
            failed_tests = parsed
                .results
                .iter()
                .flat_map(|result| result.tests.iter())
                .filter(|test| !test.passed)
                .take(MAX_FAILED_TESTS)
                .map(|test| FailedTest {
                    test: clipped(test.test.as_str(), 4096),
                    message: if test.message().trim().is_empty() {
                        "The harness rejected this named test; no failure message was supplied."
                            .into()
                    } else {
                        clipped(test.message(), MAX_MESSAGE_CHARS)
                    },
                })
                .collect();
            report = parsed
                .results
                .iter()
                .all(|r| !matches!(r.verdict, Verdict::InfrastructureFailure));
            // Q51 / #849: the parsed harness's no-report outcome is TL3-side.
            // Missing PB evidence still follows the unclassified Q7 path below.
            failure = (!report).then_some(HARNESS);
            pass = report
                && successful
                && parsed
                    .results
                    .iter()
                    .all(|r| matches!(r.verdict, Verdict::Pass));
            results = parsed
                .results
                .iter()
                .map(stored_result)
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

// Stored rows historically included the parsed report's default empty messages.
// Preserve that consumer behavior while the shared wire type retains omission.
fn stored_result(result: &CheckResult) -> serde_json::Result<serde_json::Value> {
    let mut value = serde_json::to_value(result)?;
    for (encoded, test) in value["tests"]
        .as_array_mut()
        .expect("typed test array")
        .iter_mut()
        .zip(result.tests.iter())
    {
        encoded["message"] = serde_json::json!(test.message());
    }
    Ok(value)
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

fn parse(raw: &[u8], job: &Job) -> serde_json::Result<Option<Report>> {
    let Ok(text) = std::str::from_utf8(raw) else {
        return Ok(None);
    };
    let reports: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("TL3_CHECKS "))
        .collect();
    if reports.len() != 1 {
        return Ok(None);
    }
    // #848: framing/syntax failures retain no-report; only typed decoding is a harness defect.
    let Ok(value) = serde_json::from_str::<serde_json::Value>(reports[0]) else {
        return Ok(None);
    };
    let report: Report = serde_json::from_value(value)?;
    if report.effect_key != job.delivery.key
        || report.base.as_str() != job.run.base_sha.as_str()
        || Some(report.head.as_str()) != job.delivery.head_sha.as_deref()
        || report.reviewed.as_ref().map(|sha| sha.as_str())
            != job
                .run
                .reviewed_sha
                .as_ref()
                .map(tl3_worktrees::CommitId::as_str)
    {
        return Ok(None);
    }
    if report
        .results
        .iter()
        .flat_map(|result| result.tests.iter())
        .any(|test| {
            test.phases
                .as_ref()
                .is_some_and(|phases| !phases_valid(phases, test.test.as_str()))
        })
    {
        return Ok(None);
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
    let Some(extra) = report.results.len().checked_sub(plan.len()) else {
        return Ok(None);
    };
    if extra > usize::from(job.selected_tests.is_some()) {
        return Ok(None);
    }
    if let Some(selected) = report.results.get(plan.len()) {
        let Custody::InProcess = selected.custody;
        if selected.name.as_str() != SELECTED_CHECK
            || selected.kind != Kind::Regression
            || selected.tests.len() > job.selected_tests.unwrap_or(0) as usize
        {
            return Ok(None);
        }
    }
    for (result, check) in report.results.iter().zip(&plan) {
        if result.name.as_str() != check.name
            || result.kind != check.kind
            || result.tests.len() != check.tests.len()
        {
            return Ok(None);
        }
        let Custody::InProcess = result.custody;
        if result.declared_symptom != check.declared_symptom {
            return Ok(None);
        }
        if let Some(declaration) = &check.declared_symptom {
            if !symptom_identity(result, declaration, check, job) {
                return Ok(None);
            }
        } else if result.symptom_evidence.is_some() {
            return Ok(None);
        }
        for (result, test) in result.tests.iter().zip(&check.tests) {
            if result.test.as_str() != test {
                return Ok(None);
            }
        }
        if let Some(identity) = &result.proof {
            // The receipt's rule, mode and digests are typed; these relate it to the plan.
            if !matches!(check.kind, Kind::Demonstrating | Kind::Reproducing)
                || identity.declaration != check.proof
                || identity.observer_sha256.len() != check.tests.len()
                || check.tests.iter().any(|test| {
                    !identity
                        .observer_sha256
                        .get(test)
                        .is_some_and(Option::is_some)
                })
            {
                return Ok(None);
            }
            if let Some(declaration) = &check.proof {
                let subjects = &declaration.subjects;
                let support = &declaration.observer_support;
                if (!subjects.is_empty() && identity.subjects.len() != subjects.len())
                    || subjects.iter().any(|unit| {
                        identity
                            .subjects
                            .get(unit.as_str())
                            .is_none_or(|digests| digests.base == digests.head)
                    })
                    || identity.support_sha256.len() != support.len()
                    || support
                        .iter()
                        .any(|unit| !identity.support_sha256.contains_key(unit.as_str()))
                    // Campaigns bypass judge replay but retain the old planned-proof head guard.
                    || (check.declared_symptom.is_some()
                        && result.tests.iter().any(|test| {
                            test.passed && !matches!(test.head, Outcome::Passed)
                        }))
                {
                    return Ok(None);
                }
            }
        } else if check.proof.is_some() && result.tests.iter().any(|test| test.passed) {
            return Ok(None);
        }
        // A forced check passes only on evidence from its forced head run (#393).
        if result.forcing.is_some() != check.forcing.is_some() {
            return Ok(None);
        }
    }
    Ok(Some(report))
}

fn judge_kind(kind: Kind) -> core_judge::Kind {
    match kind {
        Kind::Demonstrating => core_judge::Kind::Demonstrating,
        Kind::Reproducing => core_judge::Kind::Reproducing,
        Kind::Regression => core_judge::Kind::Regression,
        Kind::BehaviorPreserving => core_judge::Kind::BehaviorPreserving,
    }
}

fn judge_outcome(outcome: Outcome) -> core_judge::Outcome {
    match outcome {
        Outcome::Passed => core_judge::Outcome::Passed,
        Outcome::Assertion => core_judge::Outcome::Assertion,
        Outcome::AddedName => core_judge::Outcome::AddedName,
        Outcome::Error => core_judge::Outcome::Error,
        Outcome::Skipped => core_judge::Outcome::Skipped,
        Outcome::Missing => core_judge::Outcome::Missing,
        Outcome::Uncollected => core_judge::Outcome::Uncollected,
        Outcome::NoReport => core_judge::Outcome::NoReport,
    }
}

fn phase_status(outcome: PhaseOutcome) -> core_phase::Status {
    match outcome {
        PhaseOutcome::Passed => core_phase::Status::Passed,
        PhaseOutcome::Failed => core_phase::Status::Failed,
        PhaseOutcome::Skipped => core_phase::Status::Skipped,
    }
}

fn phase_agrees(run: &crate::contracts::report::PhaseRun, outcome: Outcome) -> bool {
    use core_phase::{Fact, Fold, Phase, checking};
    let folded = checking(
        phase_status(run.setup.outcome),
        run.call.as_ref().map(|call| Fact {
            phase: Phase::Call,
            status: phase_status(call.outcome),
            assertion: call.assertion,
        }),
        phase_status(run.teardown.outcome),
    );
    match (folded, outcome) {
        (Some(Fold::Passed), Outcome::Passed)
        | (Some(Fold::Failed), Outcome::Assertion)
        | (Some(Fold::Skipped), Outcome::Skipped)
        | (Some(Fold::Error), Outcome::Error) => true,
        // Missing-name candidates do not survive transport. Preserve the prior classification.
        (Some(Fold::Error), Outcome::AddedName) => {
            !matches!(run.setup.outcome, PhaseOutcome::Failed)
                && !matches!(run.teardown.outcome, PhaseOutcome::Failed)
                && run.call.as_ref().is_some_and(|call| {
                    matches!(call.outcome, PhaseOutcome::Failed) && !call.assertion
                })
        }
        _ => false,
    }
}

/// #822: replay only represented facts. Shell overrides and symptom campaigns are separate.
fn rejudged(report: &Report, job: &Job) -> bool {
    for (index, result) in report.results.iter().enumerate() {
        let check = job.run.plan.get(index);
        // These report invariants also apply to campaigns and shell overrides.
        if matches!(result.verdict, Verdict::Pass) != result.tests.iter().all(|test| test.passed)
            || result.forcing.is_some()
                && result
                    .tests
                    .iter()
                    .any(|test| test.passed && !matches!(test.head_forced, Some(Outcome::Passed)))
        {
            return false;
        }
        if result.declared_symptom.is_some() {
            continue;
        }
        // Shell overrides preserve the judge's kind/base flag and never add forcing.
        if result.tests.iter().any(|test| {
            (test.head_forced.is_some() && result.forcing.is_none())
                || core_judge::judge(core_judge::Input {
                    kind: judge_kind(result.kind),
                    base: judge_outcome(test.base),
                    head: judge_outcome(test.head),
                    forced: test.head_forced.map(judge_outcome),
                    reason: core_judge::Reason::Undeclared,
                })
                .pre_existing
                    != test.pre_existing
        }) {
            return false;
        }
        if check.is_none() {
            // Always/selected results include stale-entry, retry and group overrides.
            continue;
        }
        let mut infrastructure = false;
        let mut changed_test_refusal = false;
        for test in result.tests.iter() {
            if let Some(phases) = &test.phases {
                for (run, outcome) in [
                    (phases.base(), Some(test.base)),
                    (phases.head(), Some(test.head)),
                    (phases.head_forced(), test.head_forced),
                ] {
                    if let Some(run) = run {
                        let Some(outcome) = outcome else { return false };
                        // A late run-level rejection retains phases but replaces outcomes.
                        // TL3_CHECKS does not carry the exit/stream validity needed to replay it.
                        if !matches!(outcome, Outcome::NoReport) && !phase_agrees(run, outcome) {
                            return false;
                        }
                    }
                }
            }
            let reason = if check.is_some_and(|check| check.proof.is_some()) {
                if result
                    .proof
                    .as_ref()
                    .and_then(|proof| proof.red_message_matches.as_ref())
                    .and_then(|matches| matches.get(test.test.as_str()))
                    .is_some_and(|matched| {
                        matches!(
                            matched,
                            RedMessageMatch::Message | RedMessageMatch::SourceAssertion
                        )
                    })
                {
                    core_judge::Reason::Matched
                } else {
                    core_judge::Reason::Mismatch
                }
            } else {
                core_judge::Reason::Undeclared
            };
            let input = core_judge::Input {
                kind: judge_kind(result.kind),
                base: judge_outcome(test.base),
                head: judge_outcome(test.head),
                forced: test.head_forced.map(judge_outcome),
                reason,
            };
            let decision = core_judge::judge(input);
            infrastructure |= decision.infrastructure;
            if result.forcing.is_some() && test.head_forced.is_none() {
                if test.passed {
                    return false;
                }
                continue; // A refusal before execution has no forced-head observation.
            }
            let unknown_reason = matches!(test.base, Outcome::Assertion)
                && check.is_some_and(|check| check.proof.is_some())
                && result
                    .proof
                    .as_ref()
                    .is_some_and(|proof| proof.red_message_matches.is_none());
            let changed_test = matches!(result.kind, Kind::Regression | Kind::BehaviorPreserving)
                && job
                    .changed_paths
                    .iter()
                    .any(|path| path == test.test.as_str().split("::").next().unwrap_or(""));
            changed_test_refusal |= changed_test && !test.passed;
            if unknown_reason {
                let possible = core_judge::judge(core_judge::Input {
                    reason: core_judge::Reason::Matched,
                    ..input
                });
                if test.passed && !possible.passed {
                    return false;
                }
            } else if test.passed != decision.passed && !(changed_test && !test.passed) {
                return false;
            }
        }
        let verdict = if infrastructure {
            Verdict::InfrastructureFailure
        } else if result.tests.iter().all(|test| test.passed) {
            Verdict::Pass
        } else {
            Verdict::Fail
        };
        // execution.py's changed-test refusal replaces even InfrastructureFailure with Fail.
        // The report cannot establish that override; changed paths permit its negative result.
        if verdict != result.verdict
            && !(changed_test_refusal && matches!(result.verdict, Verdict::Fail))
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
#[path = "results/tests/mod.rs"]
mod judge_tests;

fn symptom_identity(
    result: &CheckResult,
    declaration: &crate::contracts::DeclaredSymptom,
    check: &super::Check,
    job: &Job,
) -> bool {
    let Some(evidence) = &result.symptom_evidence else {
        // Refusal before execution carries no positive proof.
        return !matches!(result.verdict, Verdict::Pass);
    };
    if evidence.base.as_str() != job.run.base_sha.as_str()
        || Some(evidence.head.as_str()) != job.delivery.head_sha.as_deref()
        || evidence.declaration != *declaration
        || evidence.forcing != check.forcing
        || check.kind != Kind::Demonstrating
        || check.tests.len() != 1
        || declaration.node.as_str() != check.tests[0]
    {
        return false;
    }
    let repeat = declaration.repeat.get();
    let mut red = 0;
    let mut green = true;
    let sides = if check.forcing.is_some() {
        &[
            SymptomSide::Base,
            SymptomSide::Head,
            SymptomSide::OrdinaryHead,
        ][..]
    } else {
        &[SymptomSide::Base, SymptomSide::Head][..]
    };
    for side in sides {
        let selected: Vec<_> = evidence
            .runs
            .iter()
            .filter(|run| run.side == *side)
            .collect();
        if selected.len() as u64 != repeat {
            return !matches!(result.verdict, Verdict::Pass);
        }
        for (index, run) in selected.iter().enumerate() {
            // The harness hashes the records' sorted, compact JSON; a Value sorts its keys.
            let Ok(encoded) =
                serde_json::to_value(&run.records).and_then(|records| serde_json::to_vec(&records))
            else {
                return false;
            };
            if run.run.get() != index as u64 + 1
                || run.sha256.as_str() != format!("{:x}", Sha256::digest(encoded))
                || run.records.is_empty()
            {
                return false;
            }
            match run.outcome {
                RunOutcome::Symptom if *side == SymptomSide::Base => red += 1,
                RunOutcome::Passed => (),
                _ => green = false,
            }
        }
    }
    evidence.runs.len() as u64 == repeat * sides.len() as u64
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
        // The receipt as execution.py::selected_base writes it.
        let receipt = json!({"policy": "historical failures first, then complete-unit prefix by import distance, path, and declaration order",
            "limit": 2, "charged_cases": 1,
            "base_units": [{"members": ["tests/test_a.py"], "cases": 1,
                "outcomes": {"tests/test_a.py": "passed"}}],
            "head_units": [["tests/test_a.py"]], "skipped_units": [["tests/test_b.py"]],
            "skipped_checked": false,
            "saturation": {"unit": ["tests/test_b.py"], "cases": 2, "remaining": 1,
                "reason": "Next complete unit cannot fit; lower-priority units were not checked."}});
        let result: super::CheckResult = serde_json::from_value(json!({
            "name": super::SELECTED_CHECK, "kind": "regression", "verdict": "pass",
            "custody": "in-process", "selection": receipt,
            "tests": [{"test": "tests/test_a.py", "base": "passed", "head": "passed",
                "pre_existing": false, "passed": true, "message": ""}],
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

/// #734: the harness's own golden reports (`harness/test-vectors/contracts`), read through
/// the controller's real result path, so a shape changed on one side alone turns red.
#[cfg(test)]
pub(crate) mod contract_vectors {
    use super::*;
    use serde_json::{Value, json};
    use std::io::Write;

    pub(crate) struct Parsed {
        pub name: String,
        pub vector: Value,
        pub report: bool,
        pub pass: bool,
        /// The stored report's results, which review sends on unchanged as `checks`.
        pub results: Vec<Value>,
        pub failed_tests: Vec<FailedTest>,
    }

    fn json_files(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut paths: Vec<_> = std::fs::read_dir(directory)
            .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect();
        paths.sort();
        paths
    }

    /// The current harness's vectors, then each released set that
    /// `harness/test-vectors/released-contracts.json` lists, with its report version:
    /// a controller resumes checks the previous release staged (#750 review finding 2).
    pub(crate) fn parsed() -> Vec<Parsed> {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../harness/test-vectors/contracts");
        let current = json_files(&directory);
        assert!(current.len() >= 7, "harness contract vectors are missing");
        let mut parsed: Vec<_> = current
            .iter()
            .map(|path| parse_vector(path, "tl3.checks.v1", ""))
            .collect();
        let manifest: Value = serde_json::from_slice(
            &std::fs::read(directory.join("../released-contracts.json")).unwrap(),
        )
        .unwrap();
        for set in manifest["sets"].as_array().unwrap() {
            let name = set["set"].as_str().unwrap();
            let paths = json_files(&directory.join("released").join(name));
            assert!(!paths.is_empty(), "{name}: no released vectors");
            parsed.extend(paths.iter().map(|path| {
                parse_vector(
                    path,
                    set["version"].as_str().unwrap(),
                    &format!("released/{name}/"),
                )
            }));
        }
        parsed
    }

    fn parse_vector(path: &std::path::Path, version: &str, prefix: &str) -> Parsed {
        let name = format!("{prefix}{}", path.file_stem().unwrap().to_string_lossy());
        let vector: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        parse_value(name, vector, version)
    }

    fn parse_value(name: String, vector: Value, version: &str) -> Parsed {
        let request = &vector["request"];
        let mut run =
            json!({"repository":"/served", "base_sha":request["base"], "plan":request["plan"]});
        if !request["reviewed"].is_null() {
            run["reviewed_sha"] = request["reviewed"].clone();
        }
        let job: Job = serde_json::from_value(json!({
            "action": "a".repeat(64),
            "delivery": {"key":request["effect_key"], "epoch":1,
                "output":{"Effect":{"node":0,"kind":"SubmitChecks","seq":1,"head":null}},
                "generation":1, "head_sha":request["head"],
                "repository":{"owner":"RobTand","name":"tl3-sandbox","installation":1},
                "issue":734, "pull_request":null, "paths":[], "attempt":1, "round":0},
            "run": run,
            "changed_paths": [],
            "selected_tests": request["selected_tests"],
            "always_tests": request.get("always_tests").cloned().unwrap_or(json!([])),
        }))
        .unwrap_or_else(|error| panic!("{name}: the vector's request is not a check job: {error}"));
        // The envelope the trusted action prints around result_json (action.py).
        let line = format!(
            "TL3_CHECKS {}\n",
            json!({"schema":version, "effect_key":request["effect_key"],
                "base":request["base"], "head":request["head"], "reviewed":request["reviewed"],
                "results":vector["results"]})
        );
        let mut log = tempfile::NamedTempFile::new().unwrap();
        log.write_all(line.as_bytes()).unwrap();
        let ending: Ending = serde_json::from_value(json!({
            "action_key": "a".repeat(64), "status":"executed", "succeeded":true, "exit_code":0,
            "result": {"verified":true, "receipt_sha256":"b".repeat(64), "stdout": {
                "path":log.path(), "bytes":line.len(),
                "sha256":format!("{:x}", Sha256::digest(line.as_bytes()))}},
        }))
        .unwrap();
        let (report, pass, results, failed_tests, failure) = evidence(ending, &job).unwrap();
        assert_eq!(failure, (!report).then_some(HARNESS), "{name}: Q51");
        Parsed {
            name,
            vector,
            report,
            pass,
            results,
            failed_tests,
        }
    }

    fn message_case(message: Option<&str>) -> Parsed {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../harness/test-vectors/contracts/head-failure.json");
        let mut vector: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let test = vector["results"][0]["tests"][0].as_object_mut().unwrap();
        match message {
            None => {
                test.remove("message");
            }
            Some(message) => {
                test.insert("message".into(), json!(message));
            }
        }
        parse_value("message-presence".into(), vector, "tl3.checks.v1")
    }

    /// #819: absence and empty text keep the same failure diagnostic and judgement.
    #[test]
    fn absent_and_empty_messages_keep_failure_diagnostics_and_judgements() {
        let absent = message_case(None);
        let empty = message_case(Some(""));
        assert!(absent.report);
        assert!(!absent.pass);
        assert_eq!((absent.report, absent.pass), (empty.report, empty.pass));
        assert_eq!(
            serde_json::to_value(&absent.failed_tests).unwrap(),
            serde_json::to_value(&empty.failed_tests).unwrap()
        );
        assert_eq!(
            absent.failed_tests[0].message,
            "The harness rejected this named test; no failure message was supplied."
        );
        assert_eq!(
            message_case(Some("failure details")).failed_tests[0].message,
            "failure details"
        );
    }

    /// #819: stored rows retain the empty default the previous parser materialized.
    #[test]
    fn absent_and_empty_messages_keep_identical_stored_result_rows() {
        let absent = message_case(None);
        let empty = message_case(Some(""));
        assert_eq!(
            serde_json::to_string(&absent.results).unwrap(),
            serde_json::to_string(&empty.results).unwrap()
        );
        assert_eq!(absent.results[0]["tests"][0]["message"], "");
        assert_eq!(
            message_case(Some("failure details")).results[0]["tests"][0]["message"],
            "failure details"
        );
    }

    #[test]
    fn controller_accepts_every_harness_vector_and_keeps_its_shape() {
        for parsed in parsed() {
            let name = &parsed.name;
            let verdicts = parsed.vector["results"].as_array().unwrap();
            assert!(
                !parsed.results.is_empty(),
                "{name}: results.rs refused the harness report; the vector and the parser disagree"
            );
            // An infrastructure verdict keeps its evidence but is not a report.
            assert_eq!(
                parsed.report,
                verdicts
                    .iter()
                    .all(|result| result["verdict"] != "infrastructure-failure"),
                "{name}"
            );
            assert_eq!(
                Value::Array(parsed.results),
                parsed.vector["results"],
                "{name}: the stored report must keep every key the harness reports, unchanged"
            );
            assert_eq!(
                parsed.pass,
                verdicts.iter().all(|result| result["verdict"] == "pass"),
                "{name}"
            );
            assert_eq!(parsed.pass, parsed.failed_tests.is_empty(), "{name}");
        }
    }
}
