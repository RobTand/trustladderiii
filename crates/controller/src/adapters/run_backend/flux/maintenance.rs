//! Retry eligibility belongs to attempted cleanup, never to launch-time debt.
use super::*;

pub(super) const CADENCE_MS: u64 = 300_000;

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn owner(record: &Record) -> Owner {
    Owner {
        owner: record.intent.owner.clone(),
        correlation: record.intent.correlation.clone(),
    }
}

impl Flux {
    pub(super) fn cleanup_deadline(&self) -> Result<Option<u64>> {
        Ok(self
            .journal
            .records()?
            .iter()
            .flat_map(|record| {
                [record.cleanup_retry_ms, record.held_submission_ms]
                    .into_iter()
                    .flatten()
            })
            .min())
    }

    pub(super) fn maintain_cleanup(&self, now: u64) -> Result<Vec<Owner>> {
        // Each record is one owner, regardless of its execution/debt count.
        // A claim advances before native I/O so restart can resume a partial pass.
        for record in self.journal.records()? {
            let owner = owner(&record);
            let claimed = self.journal.transaction(&owner, |dir, current| {
                let mut current =
                    current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
                if !current.cleanup_retry_ms.is_some_and(|due| due <= now) {
                    return Ok(false);
                }
                current.cleanup_retry_ms = Some(now.saturating_add(CADENCE_MS));
                save(dir, &current)?;
                Ok(true)
            })?;
            if claimed {
                // Errors keep the claimed deadline and debt; the pending owner
                // below raises or updates the controller incident.
                let _ = self.run_cleanup(&owner);
            }
        }
        // Cleanup runs first, so opening the gate can release held submissions
        // during this same pass. Only this explicit undispatched state resumes.
        for record in self.journal.records()? {
            let owner = owner(&record);
            let held = self.journal.transaction(&owner, |dir, current| {
                let mut current =
                    current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
                if current.held_submission_ms.is_none() {
                    return Ok(None);
                }
                if current.dispatched || current.cancel_requested || current.terminal.is_some() {
                    current.held_submission_ms = None;
                    save(dir, &current)?;
                    return Ok(None);
                }
                if !current.held_submission_ms.is_some_and(|due| due <= now) {
                    return Ok(None);
                }
                current.held_submission_ms = Some(now.saturating_add(CADENCE_MS));
                save(dir, &current)?;
                Ok(Some(current))
            })?;
            if let Some(held) = held {
                let _ = self.submit(held.request, held.intent);
            }
        }
        Ok(self
            .journal
            .records()?
            .iter()
            .filter(|record| record.cleanup_retry_ms.is_some())
            .map(owner)
            .collect())
    }
}
