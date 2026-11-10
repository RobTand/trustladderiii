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
        return judge_payload(&raw, &job.report_context(), successful);
    } else if matches!(ending.status.as_str(), "executed" | "cache_hit") {
        // A receipt-only cache hit can outlive its queue record. Missing result evidence is unknown.
        return Err(Error::Invalid(
            "successful PrismaBuild ending has no verified result",
        ));
    }
    Ok((false, false, vec![], vec![], None, None))
}

// END BODY

pub(super) fn run(ending: Ending, job: &Job) -> Result<CheckEvidence> {
    evidence(ending, job)
}
