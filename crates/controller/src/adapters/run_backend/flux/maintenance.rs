//! Retry eligibility belongs to requested cleanup, never to launch-time debt.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

pub(super) const CADENCE_MS: u64 = 300_000;

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn record_owner(record: &Record) -> Owner {
    Owner {
        owner: record.intent.owner.clone(),
        correlation: record.intent.correlation.clone(),
    }
}

impl Flux {
    fn maintenance_error(&self, key: &str, error: &str, now: u64) {
        if let Err(error) = maintenance_failure(
            &self.options.journal_root,
            key,
            error,
            now.saturating_add(CADENCE_MS),
        ) {
            // A broken error directory must not stop attempts for other owners.
            eprintln!("cannot retain maintenance failure for {key}: {error}");
        }
    }

    fn maintenance_failures_snapshot(
        &self,
        now: u64,
    ) -> (BTreeMap<String, MaintenanceFailure>, bool) {
        match maintenance_failures(&self.options.journal_root) {
            Ok(failures) => (
                failures
                    .into_iter()
                    .map(|failure| (failure.key.clone(), failure))
                    .collect(),
                false,
            ),
            Err(error) => {
                self.maintenance_error("maintenance-error-log", &error.to_string(), now);
                (BTreeMap::new(), true)
            }
        }
    }

    pub(super) fn cleanup_deadline(&self) -> Result<Option<u64>> {
        let now = now_ms();
        let (failures, error_log_failed) = self.maintenance_failures_snapshot(now);
        let mut deadlines = Vec::new();
        if error_log_failed {
            deadlines.push(now.saturating_add(CADENCE_MS));
        }
        for (key, entry) in self.journal.records()? {
            match entry {
                Ok(Some(record)) => {
                    // Committed terminals are immutable. Report unresolved debt but
                    // do not wake the Look thread for a cleanup that cannot run.
                    if record.terminal.is_some() {
                        continue;
                    }
                    let retry = failures.get(&key).map(|failure| failure.next_attempt_ms);
                    for deadline in [record.cleanup_retry_ms, record.held_submission_ms]
                        .into_iter()
                        .flatten()
                    {
                        deadlines.push(retry.map_or(deadline, |retry| retry.max(deadline)));
                    }
                }
                // A short retry wakes a busy owner without writing a failure or cadence.
                Ok(None) => deadlines.push(now.saturating_add(1_000)),
                Err(error) => {
                    let deadline = failures
                        .get(&key)
                        .map(|failure| failure.next_attempt_ms)
                        .unwrap_or_else(|| {
                            self.maintenance_error(&key, &error.to_string(), now);
                            now.saturating_add(CADENCE_MS)
                        });
                    deadlines.push(deadline);
                }
            }
        }
        Ok(deadlines.into_iter().min())
    }

    pub(super) fn maintain_cleanup(
        &self,
        now: u64,
        authorize: &dyn Fn(&Owner) -> Result<()>,
    ) -> Result<MaintenanceReport<Owner>> {
        let mut pass = self.clone();
        let deadline = Instant::now() + self.maintenance_budget;
        pass.native_deadline = Some(deadline);
        pass.maintenance_now_ms = Some(now);
        let (failures, error_log_failed) = self.maintenance_failures_snapshot(now);
        let mut records = BTreeMap::new();
        let mut pending = BTreeMap::new();
        let mut skipped_busy = false;
        if error_log_failed {
            pending.insert(
                "maintenance-error-log".into(),
                Owner {
                    owner: "maintenance-error".into(),
                    correlation: "maintenance-error-log".into(),
                },
            );
        }
        // One snapshot, reused by both phases. Reload only owners we actually touch.
        for (key, entry) in self.journal.records()? {
            match entry {
                Ok(Some(record)) => {
                    records.insert(key, record);
                }
                Ok(None) => skipped_busy = true,
                Err(error) => {
                    if failures
                        .get(&key)
                        .is_none_or(|failure| failure.next_attempt_ms <= now)
                    {
                        self.maintenance_error(&key, &error.to_string(), now);
                    }
                    pending.insert(
                        key.clone(),
                        Owner {
                            owner: "maintenance-error".into(),
                            correlation: key,
                        },
                    );
                }
            }
        }
        let mut attempted = BTreeSet::new();
        let mut failed = BTreeSet::new();
        let mut busy_claims = BTreeSet::new();
        // Cleanup first, so a released gate can resume held requests in this pass.
        for (key, record) in &mut records {
            if record.terminal.is_some() {
                if record.cleanup_retry_ms.is_some() {
                    self.maintenance_error(key, "terminal cleanup is immutable; unresolved debt requires operator investigation", now);
                }
                continue;
            }
            if !record.cleanup_retry_ms.is_some_and(|due| due <= now)
                || failures
                    .get(key)
                    .is_some_and(|failure| failure.next_attempt_ms > now)
                || attempted.len() >= self.maintenance_owners
                || Instant::now() >= deadline
            {
                continue;
            }
            let owner = record_owner(record);
            let result = (|| {
                let claimed = self.journal.try_transaction(&owner, |dir, current| {
                    let mut current =
                        current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
                    if current.terminal.is_some()
                        || !current.cleanup_retry_ms.is_some_and(|due| due <= now)
                    {
                        return Ok(false);
                    }
                    current.cleanup_retry_ms = Some(now.saturating_add(CADENCE_MS));
                    save(dir, &current)?;
                    Ok(true)
                })?;
                let Some(claimed) = claimed else {
                    return Ok(None);
                };
                if claimed {
                    attempted.insert(key.clone());
                    let result = pass.run_cleanup(&owner)?;
                    if !result.stopped {
                        return Err(BackendError::unknown(&result.reason));
                    }
                }
                self.journal.load(&owner).map(Some)
            })();
            match result {
                Ok(None) => {
                    skipped_busy = true;
                    busy_claims.insert(key.clone());
                }
                Ok(Some(current)) => {
                    *record = current;
                    if let Err(error) = clear_maintenance_failure(&self.options.journal_root, key) {
                        self.maintenance_error(key, &error.to_string(), now);
                    }
                }
                Err(error) => {
                    self.maintenance_error(key, &error.to_string(), now);
                    failed.insert(key.clone());
                    pending.insert(key.clone(), owner);
                    if let Ok(current) = self.journal.load(&record_owner(record)) {
                        *record = current;
                    }
                }
            }
        }
        for (key, record) in &mut records {
            if record.terminal.is_some() || record.held_submission_ms.is_none() {
                continue;
            }
            if !record.held_submission_ms.is_some_and(|due| due <= now)
                || failures
                    .get(key)
                    .is_some_and(|failure| failure.next_attempt_ms > now)
                || failed.contains(key)
                || busy_claims.contains(key)
                || Instant::now() >= deadline
                || (!attempted.contains(key) && attempted.len() >= self.maintenance_owners)
            {
                continue;
            }
            let owner = record_owner(record);
            let result = (|| {
                let held = self.journal.try_transaction(&owner, |dir, current| {
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
                let Some(held) = held else {
                    return Ok(None);
                };
                if let Some(held) = held {
                    attempted.insert(key.clone());
                    authorize(&owner)?;
                    match pass.submit(held.request, held.intent)? {
                        Submission::Accepted { .. } => {}
                        Submission::Refused { terminal } => {
                            return Err(BackendError::unknown(&terminal.reason));
                        }
                        Submission::Unknown { reason } => {
                            return Err(BackendError::unknown(&reason));
                        }
                    }
                }
                self.journal.load(&owner).map(Some)
            })();
            match result {
                Ok(None) => {
                    skipped_busy = true;
                    busy_claims.insert(key.clone());
                }
                Ok(Some(current)) => {
                    *record = current;
                    if let Err(error) = clear_maintenance_failure(&self.options.journal_root, key) {
                        self.maintenance_error(key, &error.to_string(), now);
                    }
                }
                Err(error) => {
                    self.maintenance_error(key, &error.to_string(), now);
                    pending.insert(key.clone(), owner);
                    if let Ok(current) = self.journal.load(&record_owner(record)) {
                        *record = current;
                    }
                }
            }
        }
        for (key, record) in records {
            if !busy_claims.contains(&key)
                && (record.cleanup_retry_ms.is_some() || record.held_submission_ms.is_some())
            {
                pending.insert(key, record_owner(&record));
            }
        }
        Ok(MaintenanceReport {
            pending: pending.into_values().collect(),
            skipped_busy,
        })
    }
}
