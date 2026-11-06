use super::*;
use std::collections::BTreeMap;

pub(super) fn ownership(record: &Record, owned: &RuntimeOwnership) -> Result<()> {
    owned.run.validate()?;
    if !token(&owned.execution) { return Err(BackendError::invalid("invalid runtime execution token")); }
    if owned.run.instance != record.intent.instance || owned.run.uri != record.intent.uri
        || record.binding.as_ref().is_some_and(|binding| binding.id != owned.run)
        || owned.owner.owner != record.intent.owner || owned.owner.correlation != record.intent.correlation
        || owned.request_digest != record.intent.request_digest || !record.dispatched
        || owned.host != "dl380g10" || owned.rank != 0 || owned.launcher.pid <= 1
        || owned.launcher.start_ticks == 0 || owned.process_group <= 1 {
        return Err(BackendError::invalid("runtime ownership differs from retained dispatch"));
    }
    match (&record.request.runtime.runtime, &owned.container) {
        (ExecutionRuntime::Native { .. } | ExecutionRuntime::RootlessPodman { .. }, None) => {}
        (ExecutionRuntime::RootlessPodman { mode, .. }, Some(container)) => {
            let labels: BTreeMap<_, _> = [
                ("tl3.backend", "flux"), ("flux.jobid", owned.run.job.as_str()), ("tl3.execution", owned.execution.as_str()), ("tl3.uri", record.intent.uri.as_str()),
                ("tl3.instance", record.intent.instance.as_str()), ("tl3.job", owned.run.job.as_str()),
                ("tl3.owner", record.intent.owner.as_str()), ("tl3.correlation", record.intent.correlation.as_str()),
                ("tl3.request_digest", record.intent.request_digest.as_str()),
            ].into_iter().map(|(key, value)| (key.to_owned(), value.to_owned())).collect();
            if container.mode != *mode || !hex(&container.id, 64) || container.labels != labels {
                return Err(BackendError::invalid("container mode/ID/ownership labels mismatch"));
            }
            match (mode, &container.scope) {
                (ContainerMode::InJob, None) => {}
                (ContainerMode::OwnScope, Some(scope)) => {
                    if !scope.unit.ends_with(".scope") || !token(&scope.unit)
                        || !hex(&scope.invocation_id, 32) || !absolute(Path::new(&scope.cgroup))
                        || !scope.cgroup.starts_with("/user.slice/user-1000.slice/user@1000.service/")
                        || !scope.cgroup.ends_with(&format!("/{}", scope.unit)) {
                        return Err(BackendError::invalid("owned scope identity is missing or unsafe"));
                    }
                }
                _ => return Err(BackendError::invalid("scope does not match selected container mode")),
            }
        }
        _ => return Err(BackendError::invalid("runtime/container ownership shape mismatch")),
    }
    Ok(())
}

pub(super) fn stopped(record: &Record, run: &BackendId, reference: &BlobRef) -> Result<bool> {
    let evidence: CleanupEvidence = serde_json::from_slice(&checked_blob(reference, &record.intent.output_dir)?)?;
    let Some(owned) = record.runtime_ownerships.iter().find(|owned| owned.execution == evidence.ownership.execution) else { return Ok(false); };
    ownership(record, owned)?;
    if evidence.schema != "tl3.cleanup_evidence.v1" || evidence.ownership != *owned || owned.run != *run
        || evidence.process_group_probe_errno != Some(3) { return Ok(false); }
    match evidence.launcher {
        ProcessStop::Absent | ProcessStop::Reaped { .. } => {}
        ProcessStop::Reused { observed_start_ticks } if observed_start_ticks != 0 && observed_start_ticks != owned.launcher.start_ticks => {}
        _ => return Ok(false),
    }
    match (&owned.container, &evidence.container) {
        (None, None) => Ok(true),
        (Some(container), Some(observed)) => {
            if observed.id != container.id || observed.remove_exit != 0 || observed.exists_exit != 1
                || observed.sweep_ids.len() > 1 || observed.sweep_ids.iter().any(|id| id != &container.id) { return Ok(false); }
            match (&container.scope, &observed.scope) {
                (None, None) => Ok(container.mode == ContainerMode::InJob),
                (Some(scope), Some(observed)) => {
                    if container.mode != ContainerMode::OwnScope || observed.identity != *scope
                        || !observed.cgroup_procs.trim().is_empty()
                        || !matches!(observed.active_state.as_str(), "inactive" | "failed" | "not-found")
                        || observed.observed_invocation_id.as_ref().is_some_and(|id| id != &scope.invocation_id) {
                        return Ok(false);
                    }
                    if let Some(raw) = &observed.cgroup_events {
                        let fields: Vec<_> = raw.lines().filter_map(|line| line.split_once(' '))
                            .filter(|(key, _)| *key == "populated").collect();
                        Ok(fields.len() == 1 && fields[0].1.trim() == "0" && raw.len() <= 4096)
                    } else {
                        // An absent cgroup must agree with an absent unit generation.
                        Ok(observed.observed_invocation_id.is_none() && observed.active_state == "not-found")
                    }
                }
                _ => Ok(false),
            }
        }
        _ => Ok(false),
    }
}

pub(super) fn all_stopped(record: &Record) -> bool {
    !record.runtime_ownerships.is_empty() && record.runtime_ownerships.iter().all(|owned| record.cleanup_evidence.get(&owned.execution)
        .is_some_and(|reference| stopped(record, &owned.run, reference).unwrap_or(false)))
}

fn result(record: &Record, accepted_execution: Option<String>, reason: &str) -> CleanupResult {
    let pending_executions = record.runtime_ownerships.iter().filter(|owned| !record.cleanup_evidence.get(&owned.execution)
        .is_some_and(|reference| stopped(record, &owned.run, reference).unwrap_or(false))).map(|owned| owned.execution.clone()).collect();
    CleanupResult { stopped: all_stopped(record), evidence: record.cleanup_evidence.values().cloned().collect(), pending_executions, accepted_execution, reason: reason.into() }
}

impl Flux {
    pub(super) fn mark_cleanup_debt(&self, record: &Record) -> Result<()> {
        self.journal.runtime_gate(|debts| {
            let runtime = debts.join(&record.request.runtime.manifest_digest);
            std::fs::create_dir_all(&runtime)?;
            let executions: Vec<_> = if record.runtime_ownerships.is_empty() { vec!["unregistered"] } else { record.runtime_ownerships.iter().map(|owned| owned.execution.as_str()).collect() };
            for execution in executions {
                atomic(&runtime.join(format!("{}.{}", record.intent.correlation, execution)), &serde_json::to_vec(&record.intent)?)?;
            }
            Ok(())
        })
    }

    pub fn retain_cleanup(&self, owner: &Owner, reference: &BlobRef) -> Result<CleanupResult> {
        let record = self.journal.load(owner)?;
        if record.terminal.is_some() { return Ok(result(&record, None, "committed terminal cleanup is immutable")); }
        self.mark_cleanup_debt(&record)?;
        let evidence: CleanupEvidence = serde_json::from_slice(&checked_blob(reference, &record.intent.output_dir)?)?;
        if !stopped(&record, &evidence.ownership.run, reference).unwrap_or(false) {
            return Ok(result(&record, None, "execution stop evidence incomplete or mismatched; dispatch held"));
        }
        let execution = evidence.ownership.execution.clone();
        self.journal.runtime_gate(|debts| {
            let current = self.journal.transaction(owner, |dir, current| {
                let mut current = current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
                if current.runtime_ownerships.iter().find(|owned| owned.execution == execution) != Some(&evidence.ownership) {
                    return Err(BackendError::unknown("execution identity changed during cleanup"));
                }
                if current.terminal.is_none() {
                    current.cleanup_evidence.insert(execution.clone(), reference.clone());
                    save(dir, &current)?;
                }
                Ok(current)
            })?;
            let runtime = debts.join(&record.request.runtime.manifest_digest);
            for owned in &current.runtime_ownerships {
                if !current.cleanup_evidence.get(&owned.execution).is_some_and(|reference| stopped(&current, &owned.run, reference).unwrap_or(false)) { continue; }
                let path = runtime.join(format!("{}.{}", record.intent.correlation, owned.execution));
                if path.exists() && serde_json::from_slice::<SubmissionIntent>(&read(&path)?)? == current.intent { std::fs::remove_file(path)?; }
            }
            if runtime.exists() { std::fs::File::open(runtime)?.sync_all()?; }
            Ok(result(&current, Some(execution), "exact execution stopped; terminal still requires every registered execution"))
        })
    }

    pub(super) fn run_cleanup(&self, owner: &Owner) -> Result<CleanupResult> {
        let record = self.journal.load(owner)?;
        if all_stopped(&record) { return Ok(result(&record, None, "all retained execution cleanup already verified")); }
        self.mark_cleanup_debt(&record)?;
        let Some(program) = &record.request.runtime.cleanup else {
            return Ok(result(&record, None, "independent trusted runtime cleanup entry unavailable; dispatch held"));
        };
        validate::trusted_artifacts(&record.request)?;
        if digest(&read(&program.executable)?) != program.executable_digest {
            return Err(BackendError::unknown("trusted cleanup executable changed; cleanup debt retained"));
        }
        for ownership in &record.runtime_ownerships {
            if record.cleanup_evidence.get(&ownership.execution).is_some_and(|reference| stopped(&record, &ownership.run, reference).unwrap_or(false)) { continue; }
            let cleaned = (|| -> Result<()> {
                let input = serde_json::to_vec(&CleanupInput { schema: "tl3.cleanup_input.v1".into(), ownership: ownership.clone() })?;
                // Deliberately uses retained labels/generation after broker or launcher loss.
                let mut command = std::process::Command::new(&program.executable);
                command.args(&program.arguments).env_clear().envs(&record.request.environment);
                let reply = self.capture_command(command, Some(input))?;
                blob(&record.intent.output_dir.join(format!("cleanup-{}.stderr", ownership.execution)), safe(&String::from_utf8_lossy(&reply.stderr))?.as_bytes())?;
                if !reply.success { return Err(BackendError::unknown("independent execution cleanup failed")); }
                let evidence: CleanupEvidence = serde_json::from_slice(&reply.stdout)?;
                if evidence.ownership != *ownership { return Err(BackendError::invalid("cleanup helper returned another execution")); }
                let reference = blob(&record.intent.output_dir.join(format!("cleanup-{}.json", digest(&reply.stdout))), &reply.stdout)?;
                self.retain_cleanup(owner, &reference)?;
                Ok(())
            })();
            // Keep attempting other exact owned executions; debt is the durable failure result.
            if cleaned.is_err() { continue; }
        }
        Ok(result(&self.journal.load(owner)?, None, "independent cleanup attempted every retained execution; unresolved debt remains held"))
    }
}
