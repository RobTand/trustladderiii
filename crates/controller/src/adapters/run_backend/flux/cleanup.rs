use super::*;
use std::collections::BTreeMap;

pub(super) fn ownership(record: &Record, owned: &RuntimeOwnership) -> Result<()> {
    owned.run.validate()?;
    if owned.run.instance != record.intent.instance || owned.run.uri != record.intent.uri
        || record.binding.as_ref().is_some_and(|binding| binding.id != owned.run)
        || owned.owner.owner != record.intent.owner || owned.owner.correlation != record.intent.correlation
        || owned.request_digest != record.intent.request_digest || !record.dispatched
        || owned.host != "dl380g10" || owned.rank != 0 || owned.launcher.pid <= 1
        || owned.launcher.start_ticks == 0 || owned.process_group <= 1 {
        return Err(BackendError::invalid("runtime ownership differs from retained dispatch"));
    }
    match (&record.request.runtime.runtime, &owned.container) {
        (ExecutionRuntime::Native { .. }, None) => {}
        (ExecutionRuntime::RootlessPodman { mode, .. }, Some(container)) => {
            let labels: BTreeMap<_, _> = [
                ("tl3.backend", "flux"), ("tl3.uri", record.intent.uri.as_str()),
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
                    let prefix = format!("tl3-{}-{}-", record.intent.correlation, owned.run.job);
                    if !scope.unit.starts_with(&prefix) || !scope.unit.ends_with(".scope") || !token(&scope.unit)
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
    let Some(owned) = &record.runtime_ownership else { return Ok(false); };
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

impl Flux {
    pub(super) fn mark_cleanup_debt(&self, record: &Record) -> Result<()> {
        self.journal.runtime_gate(|debts| {
            let runtime = debts.join(&record.request.runtime.manifest_digest);
            std::fs::create_dir_all(&runtime)?;
            atomic(&runtime.join(&record.intent.correlation), &serde_json::to_vec(&record.intent)?)
        })
    }

    pub fn retain_cleanup(&self, owner: &Owner, reference: &BlobRef) -> Result<CleanupResult> {
        let record = self.journal.load(owner)?;
        self.mark_cleanup_debt(&record)?;
        let Some(owned) = &record.runtime_ownership else {
            return Ok(CleanupResult { stopped: false, evidence: None, reason: "runtime ownership was not durably retained before execution".into() });
        };
        if !stopped(&record, &owned.run, reference).unwrap_or(false) {
            return Ok(CleanupResult { stopped: false, evidence: None, reason: "owned process/container/scope stop evidence incomplete or mismatched; dispatch held".into() });
        }
        self.journal.runtime_gate(|debts| {
            self.journal.transaction(owner, |dir, current| {
                let mut current = current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
                if current.runtime_ownership != record.runtime_ownership || current.binding != record.binding {
                    return Err(BackendError::unknown("runtime identity changed during cleanup"));
                }
                current.cleanup_evidence = Some(reference.clone());
                save(dir, &current)
            })?;
            let path = debts.join(&record.request.runtime.manifest_digest).join(&record.intent.correlation);
            // A different owner may also have unresolved cleanup. Never erase their debt.
            if path.exists() && serde_json::from_slice::<SubmissionIntent>(&read(&path)?)? == record.intent {
                std::fs::remove_file(&path)?;
                std::fs::File::open(path.parent().ok_or_else(|| BackendError::invalid("cleanup debt parent missing"))?)?.sync_all()?;
            }
            Ok(())
        })?;
        Ok(CleanupResult { stopped: true, evidence: Some(reference.clone()), reason: "retained exact ownership and authoritative stopped observations agree".into() })
    }

    pub(super) fn run_cleanup(&self, owner: &Owner) -> Result<CleanupResult> {
        let record = self.journal.load(owner)?;
        self.mark_cleanup_debt(&record)?;
        let Some(ownership) = record.runtime_ownership.clone() else {
            return Ok(CleanupResult { stopped: false, evidence: None, reason: "no retained runtime ownership; cleanup remains unknown".into() });
        };
        let Some(program) = &record.request.runtime.cleanup else {
            return Ok(CleanupResult { stopped: false, evidence: None, reason: "independent trusted runtime cleanup entry unavailable; dispatch held".into() });
        };
        // Cleanup deliberately uses retained identity even after broker generation loss.
        // The helper must cross-check exact labels and scope generation before removal.
        if digest(&read(&program.executable)?) != program.executable_digest {
            return Err(BackendError::unknown("trusted cleanup executable changed; cleanup debt retained"));
        }
        let input = serde_json::to_vec(&CleanupInput { schema: "tl3.cleanup_input.v1".into(), ownership })?;
        let mut command = std::process::Command::new(&program.executable);
        command.args(&program.arguments).env_clear().envs(&record.request.environment);
        let reply = self.capture_command(command, Some(input))?;
        blob(&record.intent.output_dir.join("cleanup.stderr"), safe(&String::from_utf8_lossy(&reply.stderr))?.as_bytes())?;
        if !reply.success {
            return Ok(CleanupResult { stopped: false, evidence: None, reason: "independent runtime cleanup failed; debt retained".into() });
        }
        let stdout = reply.stdout;
        let _: CleanupEvidence = serde_json::from_slice(&stdout)?;
        let reference = blob(&record.intent.output_dir.join("cleanup.json"), &stdout)?;
        self.retain_cleanup(owner, &reference)
    }
}
