use super::*;
use serde::{Deserialize, Serialize};

/// The existing helper attribution for externally attributed scratch reclamation.
const NO_GRANT_RECLAMATION: &str = "external scratch reclamation";

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ScratchInput<'a> {
    schema: &'static str,
    run: &'a BackendId,
    owner: Owner,
    request_digest: &'a str,
    host: &'a str,
    nonce: &'a str,
    budget_seconds: u64,
    reclamation: Option<&'a str>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Generation {
    #[serde(rename = "exit_code")]
    _exit_code: i32,
    stdout: String,
    stderr: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ScratchState {
    Absent {
        errno: i32,
    },
    Present,
    Unknown {
        #[serde(rename = "reason")]
        _reason: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScratchObservation {
    schema: String,
    run: BackendId,
    owner: Owner,
    request_digest: String,
    host: String,
    nonce: String,
    path: PathBuf,
    uid: u32,
    generation_before: Generation,
    generation_pre_removal: Option<Generation>,
    generation_after: Generation,
    started_monotonic_ns: u64,
    finished_monotonic_ns: u64,
    observation: ScratchState,
    attribution: Option<String>,
    manifest: Option<PathBuf>,
}

impl Flux {
    pub(super) fn commit_scratch_terminal(
        &self,
        owner: &Owner,
        record: &Record,
        reclamation: Option<&str>,
        classification: (Ending, &str),
        mut evidence: Vec<BlobRef>,
        report: Option<BlobRef>,
    ) -> Result<Option<Terminal>> {
        self.journal.runtime_gate(|debts| {
            // begin_launch takes this gate before its journal transaction. Keep
            // it through terminal commit so a waiting begin sees the terminal.
            let current = self.journal.load(owner)?;
            if serde_json::to_vec(&current)? != serde_json::to_vec(record)? {
                return Ok(None);
            }
            let dir = debts.join(&record.request.runtime.manifest_digest);
            std::fs::create_dir_all(&dir)?;
            atomic(
                &dir.join(format!("{}.scratch", record.intent.correlation)),
                &serde_json::to_vec(&record.intent)?,
            )?;
            let Some(fresh) = self.terminal_scratch_absence(record, reclamation)? else {
                return Ok(None);
            };
            evidence.extend(fresh);
            self.terminal(owner, classification.0, classification.1, evidence, report)
                .map(Some)
        })
    }

    pub(super) fn scratch_debt(&self, record: &Record) -> Result<()> {
        self.journal.runtime_gate(|debts| {
            let dir = debts.join(&record.request.runtime.manifest_digest);
            std::fs::create_dir_all(&dir)?;
            atomic(
                &dir.join(format!("{}.scratch", record.intent.correlation)),
                &serde_json::to_vec(&record.intent)?,
            )
        })
    }

    pub(super) fn retire_scratch_debt(&self, owner: &Owner, terminal: &Terminal) -> Result<()> {
        self.journal.runtime_gate(|debts| {
            let record = self.journal.load(owner)?;
            let dir = debts.join(&record.request.runtime.manifest_digest);
            let path = dir.join(format!("{}.scratch", record.intent.correlation));
            if !path.exists() {
                return Ok(());
            }
            if record.terminal.as_ref() != Some(terminal)
                || serde_json::from_slice::<SubmissionIntent>(&read(&path)?)? != record.intent
            {
                return Err(BackendError::unknown(
                    "scratch debt lacks matching committed terminal",
                ));
            }
            let receipt: Receipt = serde_json::from_slice(&checked_blob(
                &terminal.receipt,
                &record.intent.output_dir,
            )?)?;
            if !receipt.evidence.iter().any(|r| {
                r.path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("scratch-absence-"))
            }) {
                return Err(BackendError::unknown(
                    "scratch debt lacks accepted absence evidence",
                ));
            }
            std::fs::remove_file(path)?;
            std::fs::File::open(dir)?.sync_all()?;
            Ok(())
        })
    }

    fn terminal_scratch_absence(
        &self,
        record: &Record,
        reclamation: Option<&str>,
    ) -> Result<Option<Vec<BlobRef>>> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        if !record.dispatched {
            return Ok(Some(vec![]));
        }
        if !record
            .events
            .iter()
            .any(|e| matches!(e.fact, NativeFact::Clean))
            || !cleanup::all_stopped(record)
        {
            return Ok(None);
        }
        self.instance(record)?;
        validate::trusted_artifacts(&record.request)?;
        let Some(program) = &record.request.runtime.cleanup else {
            return Ok(None);
        };
        if digest(&read(&program.executable)?) != program.executable_digest {
            return Ok(None);
        }
        let Some(binding) = &record.binding else {
            return Ok(None);
        };
        if canonical_job_id(&binding.id.job)? != binding.id.job {
            return Ok(None);
        }
        let (host, reclamation) = match &record.allocation_grant {
            Some(grant) => {
                if grant.run != binding.id
                    || grant.request_digest != record.intent.request_digest
                    || grant.host.is_empty()
                    || grant.rank != 0
                    || record.container_launches.values().any(|launch| {
                        launch.launcher.host != grant.host || launch.launcher.run != grant.run
                    })
                    || record
                        .runtime_ownerships
                        .iter()
                        .any(|owned| owned.host != grant.host || owned.run != grant.run)
                {
                    return Ok(None);
                }
                (grant.host.as_str(), reclamation)
            }
            // A runner that never validated its allocation recorded no grant and no
            // execution. The digest-bound first-cut placement is the only host the
            // job could reach; the pinned helper confirms it by its own nodename and
            // reclaims only the exact retained leaf through its guarded flow.
            None => {
                if !record.runtime_ownerships.is_empty()
                    || !record.container_launches.is_empty()
                    || !record.cleanup_evidence.is_empty()
                    || record
                        .request
                        .runtime
                        .host_dependency
                        .as_ref()
                        .is_none_or(|dependency| dependency.host != validate::FIRST_CUT_HOST)
                {
                    return Ok(None);
                }
                (validate::FIRST_CUT_HOST, Some(NO_GRANT_RECLAMATION))
            }
        };
        let nonce = digest(
            format!(
                "{:?}:{}:{}",
                std::time::SystemTime::now(),
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )
            .as_bytes(),
        );
        let input = serde_json::to_vec(&ScratchInput {
            schema: "tl3.scratch_cleanup_input.v1",
            run: &binding.id,
            owner: Owner {
                owner: record.intent.owner.clone(),
                correlation: record.intent.correlation.clone(),
            },
            request_digest: &record.intent.request_digest,
            host,
            nonce: &nonce,
            budget_seconds: 5,
            reclamation,
        })?;
        let out = &record.intent.output_dir;
        let input_ref = blob(
            &out.join(format!("scratch-absence-{nonce}.input.json")),
            &input,
        )?;
        let mut command = std::process::Command::new(&program.executable);
        command
            .args(&program.arguments)
            .env_clear()
            .envs(&record.request.environment);
        let reply = self.capture_command(command, Some(input))?;
        let raw = blob(
            &out.join(format!("scratch-absence-{nonce}.stdout")),
            &reply.stdout,
        )?;
        let stderr = blob(
            &out.join(format!("scratch-absence-{nonce}.stderr")),
            safe(&String::from_utf8_lossy(&reply.stderr))?.as_bytes(),
        )?;
        let status = blob(
            &out.join(format!("scratch-absence-{nonce}.status.json")),
            &serde_json::to_vec(&reply.status)?,
        )?;
        let observed: ScratchObservation = match serde_json::from_slice(&reply.stdout) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        let bounded_observation = |g: &Generation| {
            g.stdout.len() <= 16384 && g.stderr.len() <= 16384
        };
        if !reply.success
            || observed.schema != "tl3.scratch_cleanup.v1"
            || observed.run != binding.id
            || observed.owner.owner != record.intent.owner
            || observed.owner.correlation != record.intent.correlation
            || observed.request_digest != record.intent.request_digest
            || observed.host != host
            || record
                .runtime_ownerships
                .iter()
                .any(|o| o.host != observed.host || o.run != observed.run)
            || observed.nonce != nonce
            || observed.path != record.request.scratch_root.join(&binding.id.job)
            || observed.uid != 1000
            || !matches!(observed.observation, ScratchState::Absent { errno: 2 })
            || !bounded_observation(&observed.generation_before)
            || !bounded_observation(&observed.generation_after)
            || observed.started_monotonic_ns == 0
            || observed.finished_monotonic_ns < observed.started_monotonic_ns
            || observed.finished_monotonic_ns - observed.started_monotonic_ns > 10_000_000_000
        {
            return Ok(None);
        }
        let mut evidence = vec![input_ref, raw, stderr, status];
        match (&observed.attribution, &observed.manifest) {
            (None, None) if observed.generation_pre_removal.is_none() => {}
            (Some(attribution), Some(path))
                if Some(attribution.as_str()) == reclamation
                    && observed
                        .generation_pre_removal
                        .as_ref()
                        .is_some_and(bounded_observation)
                    && *path == out.join("scratch-reclamation.manifest.json") =>
            {
                let bytes = read(path)?;
                let manifest: serde_json::Value = serde_json::from_slice(&bytes)?;
                if manifest["schema"] != "tl3.scratch_manifest.v1"
                    || manifest["path"] != observed.path.to_string_lossy().as_ref()
                    || manifest["uid"] != observed.uid
                    || manifest["run"] != serde_json::to_value(&binding.id)?
                    || manifest["owner"] != serde_json::to_value(&observed.owner)?
                    || manifest["request_digest"] != record.intent.request_digest
                    || manifest["attribution"] != *attribution
                    || manifest["entries"]
                        .as_array()
                        .is_none_or(|entries| entries.len() > 4096)
                    || manifest["total_bytes"]
                        .as_u64()
                        .is_none_or(|n| n > 2 * 1024 * 1024 * 1024)
                {
                    return Ok(None);
                }
                evidence.push(blob(path, &bytes)?);
                let completion = out.join("scratch-reclamation.complete.json");
                let completed = read(&completion)?;
                if serde_json::from_slice::<serde_json::Value>(&completed)?
                    != serde_json::from_slice::<serde_json::Value>(&reply.stdout)?
                {
                    return Ok(None);
                }
                evidence.push(blob(&completion, &completed)?);
            }
            _ => return Ok(None),
        }
        validate::trusted_artifacts(&record.request)?;
        if digest(&read(&program.executable)?) != program.executable_digest {
            return Ok(None);
        }
        let latest = self.journal.load(&Owner {
            owner: record.intent.owner.clone(),
            correlation: record.intent.correlation.clone(),
        })?;
        self.instance(&latest)?;
        if serde_json::to_vec(&latest)? != serde_json::to_vec(record)? {
            return Ok(None);
        }
        Ok(Some(evidence))
    }
}
