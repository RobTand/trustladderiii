// Test-only pre-extraction reference from cdacce8f99c28ca8dba8f5f76831b77b4dfcd836.
// Function bodies are verbatim; File is the recorded I/O implementation.
use super::super::*;
use super::recorded_io::File;

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
        // The typed report clips each message to MAX_MESSAGE_CHARS as it parses.
        if let Some(parsed) = parse(&raw, job) {
            failed_tests = parsed
                .results
                .iter()
                .flat_map(|result| result.tests.iter())
                .filter(|test| !test.passed)
                .take(MAX_FAILED_TESTS)
                .map(|test| FailedTest {
                    test: clipped(test.test.as_str(), 4096),
                    message: if test.message.as_str().trim().is_empty() {
                        "The harness rejected this named test; no failure message was supplied."
                            .into()
                    } else {
                        clipped(test.message.as_str(), MAX_MESSAGE_CHARS)
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
        return None;
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
        if selected.name.as_str() != SELECTED_CHECK
            || selected.kind != Kind::Regression
            || selected.tests.len() > job.selected_tests.unwrap_or(0) as usize
            || matches!(selected.verdict, Verdict::Pass) != selected.tests.iter().all(|t| t.passed)
        {
            return None;
        }
    }
    for (result, check) in report.results.iter().zip(&plan) {
        if result.name.as_str() != check.name
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
            if result.test.as_str() != test {
                return None;
            }
        }
        if matches!(result.verdict, Verdict::Pass) != result.tests.iter().all(|t| t.passed) {
            return None;
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
                return None;
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
                    || result.tests.iter().any(|test| {
                        test.passed
                            && ((check.declared_symptom.is_none()
                                && !matches!(test.base, Outcome::Assertion | Outcome::AddedName))
                                || !matches!(test.head, Outcome::Passed))
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
                .any(|test| test.passed && matches!(test.base, Outcome::Assertion))
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

pub(super) fn run(ending: Ending, job: &Job) -> Result<CheckEvidence> {
    evidence(ending, job)
}
