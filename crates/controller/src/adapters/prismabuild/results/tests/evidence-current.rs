// Current production evidence shell. before.rs remains the frozen extraction reference.
use super::super::*;
use super::recorded_io::File;

// BEGIN BODY
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
    let mut diagnostic = None;
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
                return Ok((false, false, vec![], vec![diagnostic], Some(HARNESS), None));
            }
        };
        if let Some(parsed) = parsed {
            if !rejudged(&parsed, job) {
                return Ok((false, false, vec![], vec![], JUDGE_MISMATCH_CLASS, None));
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
            // Q51 / #849: a successful PB action's no-report outcome is TL3-side.
            // Known PB failures retain the unclassified Q7 path, even with results.
            failure = (successful && !report).then_some(HARNESS);
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
        if !report {
            diagnostic = crate::check_execution::from_log(&raw);
        }
    } else if matches!(ending.status.as_str(), "executed" | "cache_hit") {
        // A receipt-only cache hit can outlive its queue record. Missing result evidence is unknown.
        return Err(Error::Invalid(
            "successful PrismaBuild ending has no verified result",
        ));
    }
    Ok((report, pass, results, failed_tests, failure, diagnostic))
}

// END BODY

pub(super) fn run(ending: Ending, job: &Job) -> Result<CheckEvidence> {
    evidence(ending, job)
}
