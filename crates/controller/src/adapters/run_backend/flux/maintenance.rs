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
        let now = now_ms();
        let mut due = None;
        let failures = maintenance_failures(&self.options.journal_root)?;
        for (key, entry) in self.journal.records()? {
            match entry {
                Ok(record) => {
                    let retry = failures
                        .iter()
                        .find(|failure| failure.key == key)
                        .map(|failure| failure.next_attempt_ms);
                    for deadline in [record.cleanup_retry_ms, record.held_submission_ms]
                        .into_iter()
                        .flatten()
                    {
                        let deadline = retry.map_or(deadline, |retry| retry.max(deadline));
                        due = Some(due.map_or(deadline, |current: u64| current.min(deadline)));
                    }
                }
                Err(error) => {
                    let deadline =
                        if let Some(failure) = failures.iter().find(|failure| failure.key == key) {
                            failure.next_attempt_ms
                        } else {
                            let deadline = now.saturating_add(CADENCE_MS);
                            maintenance_failure(
                                &self.options.journal_root,
                                &key,
                                &error.to_string(),
                                deadline,
                            )?;
                            deadline
                        };
                    due = Some(due.map_or(deadline, |current: u64| current.min(deadline)));
                }
            }
        }
        Ok(due)
    }

    pub(super) fn maintain_cleanup(&self, now: u64) -> Result<Vec<Owner>> {
        let mut pending = Vec::new();
        // One scan per pass, retaining failures alongside readable owner records.
        let records = self.journal.records()?;
        for (key, entry) in records {
            let record = match entry {
                Ok(record) => record,
                Err(error) => {
                    maintenance_failure(
                        &self.options.journal_root,
                        &key,
                        &error.to_string(),
                        now.saturating_add(CADENCE_MS),
                    )?;
                    pending.push(Owner {
                        owner: "maintenance-error".into(),
                        correlation: key,
                    });
                    continue;
                }
            };
            let owner = owner(&record);
            let attempt = (|| {
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
                    self.run_cleanup(&owner)?;
                }
                let held = self.journal.transaction(&owner, |dir, current| {
                    let mut current =
                        current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
                    if current.held_submission_ms.is_none() {
                        return Ok(None);
                    }
                    if current.dispatched || current.cancel_requested || current.terminal.is_some()
                    {
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
                    self.submit(held.request, held.intent)?;
                }
                self.journal.load(&owner)
            })();
            match attempt {
                Ok(current) => {
                    clear_maintenance_failure(&self.options.journal_root, &key)?;
                    if current.cleanup_retry_ms.is_some() {
                        pending.push(owner);
                    }
                }
                Err(error) => {
                    maintenance_failure(
                        &self.options.journal_root,
                        &key,
                        &error.to_string(),
                        now.saturating_add(CADENCE_MS),
                    )?;
                    pending.push(owner);
                }
            }
        }
        Ok(pending)
    }
}
