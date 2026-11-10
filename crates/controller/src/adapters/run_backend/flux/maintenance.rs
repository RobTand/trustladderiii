//! Retry eligibility belongs to requested cleanup, never to launch-time debt.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

pub(super) const CADENCE_MS: u64 = 300_000;

#[derive(Default)]
pub(super) struct MaintenanceState {
    last_pass_ms: Option<u64>,
    last_error_print_ms: Option<u64>,
}

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
            let mut state = self
                .maintenance_state
                .lock()
                .expect("maintenance state mutex");
            if state
                .last_error_print_ms
                .is_none_or(|last| now.saturating_sub(last) >= CADENCE_MS)
            {
                state.last_error_print_ms = Some(now);
                drop(state);
                eprintln!("cannot retain maintenance failure for {key}: {error}");
            }
        }
    }

    fn busy_owner(&self, key: &str, now: u64) -> Result<(Owner, bool)> {
        let first = maintenance_busy_since(&self.options.journal_root, key, now)?;
        let owner = self.journal.owner_identity(key)?;
        Ok((
            owner,
            now.saturating_sub(first) >= self.maintenance_busy_bound_ms,
        ))
    }

    fn acquired_owner(
        &self,
        key: &str,
        failures: &mut BTreeMap<String, MaintenanceFailure>,
    ) -> Result<()> {
        clear_maintenance_busy(&self.options.journal_root, key)?;
        if failures
            .get(key)
            .is_some_and(|failure| failure.message.starts_with("persistently held owner lock"))
        {
            clear_maintenance_failure(&self.options.journal_root, key)?;
            failures.remove(key);
        }
        Ok(())
    }

    fn report_unreadable_owner(&self, key: &str, pending: &mut BTreeMap<String, Owner>) {
        // Debt retains the last-known identity independently of state.json. This
        // read preserves incidents only; it cannot establish stop evidence.
        let debts = self.options.journal_root.join("cleanup-debt");
        if !debts.is_symlink() {
            for runtime in std::fs::read_dir(&debts).into_iter().flatten().flatten() {
                let runtime = runtime.path();
                if runtime.is_symlink() || !runtime.is_dir() {
                    continue;
                }
                for debt in std::fs::read_dir(runtime).into_iter().flatten().flatten() {
                    let path = debt.path();
                    if path.is_symlink()
                        || !path.file_name().is_some_and(|name| {
                            name.to_str()
                                .is_some_and(|name| name.starts_with(&format!("{key}.")))
                        })
                    {
                        continue;
                    }
                    let intent = read(&path)
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<SubmissionIntent>(&bytes).ok());
                    if let Some(intent) =
                        intent.filter(|intent| intent.correlation == key && token(&intent.owner))
                    {
                        pending.insert(
                            format!("{key}:{}", intent.owner),
                            Owner {
                                owner: intent.owner,
                                correlation: intent.correlation,
                            },
                        );
                    }
                }
            }
        }
        // Keep the diagnostic incident too. With no readable retained identity,
        // only this surrogate can be named; guessing could close the wrong owner.
        pending.insert(
            key.into(),
            Owner {
                owner: "maintenance-error".into(),
                correlation: key.into(),
            },
        );
    }

    fn report_busy(
        &self,
        key: &str,
        now: u64,
        failures: &BTreeMap<String, MaintenanceFailure>,
        pending: &mut BTreeMap<String, Owner>,
        unobserved: &mut BTreeMap<String, Owner>,
    ) {
        match self.busy_owner(key, now) {
            Ok((owner, expired)) => {
                unobserved.insert(key.into(), owner.clone());
                if expired
                    || failures.get(key).is_some_and(|failure| {
                        failure.message.starts_with("persistently held owner lock")
                    })
                {
                    if failures
                        .get(key)
                        .is_none_or(|failure| failure.next_attempt_ms <= now)
                    {
                        self.maintenance_error(
                            key,
                            "persistently held owner lock; cleanup is unproven",
                            now,
                        );
                    }
                    pending.insert(key.into(), owner);
                }
            }
            Err(error) => {
                self.maintenance_error(key, &error.to_string(), now);
                self.report_unreadable_owner(key, pending);
            }
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

    fn failure_deadline(&self, due: u64, now: u64) -> u64 {
        if due > now {
            return due;
        }
        self.maintenance_state
            .lock()
            .expect("maintenance state mutex")
            .last_pass_ms
            .map_or(due, |last| due.max(last.saturating_add(CADENCE_MS)))
    }

    fn owner_error_deadline(
        &self,
        key: &str,
        error: &BackendError,
        now: u64,
        failures: &BTreeMap<String, MaintenanceFailure>,
    ) -> u64 {
        failures
            .get(key)
            .map(|failure| self.failure_deadline(failure.next_attempt_ms, now))
            .unwrap_or_else(|| {
                self.maintenance_error(key, &error.to_string(), now);
                now.saturating_add(CADENCE_MS)
            })
    }

    pub(super) fn cleanup_deadline(&self) -> Result<Option<u64>> {
        let now = now_ms();
        let (mut failures, error_log_failed) = self.maintenance_failures_snapshot(now);
        let mut deadlines = Vec::new();
        if error_log_failed {
            deadlines.push(now.saturating_add(CADENCE_MS));
        }
        for (key, entry) in self.journal.records()? {
            match entry {
                Ok(Some(record)) => {
                    if let Err(error) = self.acquired_owner(&key, &mut failures) {
                        deadlines.push(self.owner_error_deadline(&key, &error, now, &failures));
                        continue;
                    }
                    // Committed terminals are immutable. Report unresolved debt but
                    // do not wake the Look thread for a cleanup that cannot run.
                    if record.terminal.is_some() {
                        continue;
                    }
                    let retry = failures
                        .get(&key)
                        .map(|failure| self.failure_deadline(failure.next_attempt_ms, now));
                    for deadline in [record.cleanup_retry_ms, record.held_submission_ms]
                        .into_iter()
                        .flatten()
                    {
                        deadlines.push(retry.map_or(deadline, |retry| retry.max(deadline)));
                    }
                }
                Ok(None) => {
                    let expired = match self.busy_owner(&key, now) {
                        Ok((_, expired)) => expired,
                        Err(error) => {
                            deadlines.push(self.owner_error_deadline(&key, &error, now, &failures));
                            continue;
                        }
                    };
                    if expired
                        || failures.get(&key).is_some_and(|failure| {
                            failure.message.starts_with("persistently held owner lock")
                        })
                    {
                        // Keep the first pass due, then back off even if the pass
                        // cannot rewrite a readable retained failure.
                        deadlines.push(
                            failures
                                .get(&key)
                                .map_or(now.saturating_add(CADENCE_MS), |failure| {
                                    self.failure_deadline(failure.next_attempt_ms, now)
                                }),
                        );
                    } else {
                        deadlines.push(now.saturating_add(1_000));
                    }
                }
                Err(error) => {
                    deadlines.push(self.owner_error_deadline(&key, &error, now, &failures));
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
        let (mut failures, error_log_failed) = self.maintenance_failures_snapshot(now);
        let mut records = BTreeMap::new();
        let mut pending = BTreeMap::new();
        let mut skipped_busy = false;
        let mut unobserved = BTreeMap::new();
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
                    if let Err(error) = self.acquired_owner(&key, &mut failures) {
                        self.maintenance_error(&key, &error.to_string(), now);
                        pending.insert(key.clone(), record_owner(&record));
                    }
                    records.insert(key, record);
                }
                Ok(None) => {
                    skipped_busy = true;
                    self.report_busy(&key, now, &failures, &mut pending, &mut unobserved);
                }
                Err(error) => {
                    if failures
                        .get(&key)
                        .is_none_or(|failure| failure.next_attempt_ms <= now)
                    {
                        self.maintenance_error(&key, &error.to_string(), now);
                    }
                    self.report_unreadable_owner(&key, &mut pending);
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
                    self.report_busy(key, now, &failures, &mut pending, &mut unobserved);
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
                    self.report_busy(key, now, &failures, &mut pending, &mut unobserved);
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
        self.maintenance_state
            .lock()
            .expect("maintenance state mutex")
            .last_pass_ms = Some(now);
        Ok(MaintenanceReport {
            pending: pending.into_values().collect(),
            skipped_busy,
            unobserved: unobserved.into_values().collect(),
        })
    }
}
