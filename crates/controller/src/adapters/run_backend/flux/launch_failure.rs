use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LauncherExit {
    execution: String,
    exit_code: Option<i32>,
    command_released: bool,
    natural_exit: bool,
    monotonic_ns: u64,
    stderr_tail: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoContainerObserved {
    execution: String,
    cleanup: BlobRef,
    pidfd_cessation: bool,
}

fn event<T: serde::de::DeserializeOwned>(
    rows: &[serde_json::Value],
    kind: &str,
) -> Result<(usize, T)> {
    let mut matching = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row["event"] == kind);
    let (index, row) = matching
        .next()
        .ok_or_else(|| BackendError::unknown("required process evidence missing"))?;
    if matching.next().is_some() {
        return Err(BackendError::unknown("duplicate process evidence"));
    }
    let mut row = row.clone();
    row.as_object_mut()
        .ok_or_else(|| BackendError::invalid("process evidence is not an object"))?
        .remove("event");
    Ok((index, serde_json::from_value(row)?))
}

fn local(record: &Record, proof: &LaunchFailureEvidence) -> Result<bool> {
    let Some(intent) = record.container_launches.get(&proof.execution) else {
        return Ok(false);
    };
    let out = &record.intent.output_dir;
    if proof.process_record.path != out.join(format!("{}.runtime.jsonl", proof.execution))
        || record.cleanup_evidence.get(&proof.execution) != Some(&proof.cleanup)
        || intent.labels != super::lifecycle::labels(&intent.launcher)
        || intent.labels.len() != 9
        || !super::cleanup::stopped(record, &intent.launcher.run, &proof.cleanup)?
    {
        return Ok(false);
    }
    let raw = checked_blob(&proof.process_record, out)?;
    let rows: Vec<serde_json::Value> = raw
        .split(|b| *b == b'\n')
        .filter(|row| !row.is_empty())
        .map(serde_json::from_slice)
        .collect::<std::result::Result<_, _>>()?;
    if rows.iter().any(|row| {
        matches!(
            row["event"].as_str(),
            Some(
                "container"
                    | "container_processes"
                    | "launch_binding"
                    | "qualification"
                    | "launch_uncertain"
            )
        )
    }) {
        return Ok(false);
    }
    let (begin_index, begin): (_, ContainerLaunchIntent) = event(&rows, "launch_intent")?;
    let (exit_index, exit): (_, LauncherExit) = event(&rows, "launcher_exit")?;
    let (absence_index, absence): (_, NoContainerObserved) = event(&rows, "no_container_observed")?;
    let cleanup: CleanupEvidence = serde_json::from_slice(&checked_blob(&proof.cleanup, out)?)?;
    let Some(pending) = cleanup.pending_launch else {
        return Ok(false);
    };
    Ok(begin == *intent
        && begin_index < exit_index
        && exit_index < absence_index
        && exit.execution == proof.execution
        && exit.exit_code.is_some_and(|code| code != 0)
        && exit.command_released
        && exit.natural_exit
        && exit.monotonic_ns > 0
        && exit.stderr_tail.len() <= 4096
        && safe(&exit.stderr_tail)? == exit.stderr_tail
        && absence.execution == proof.execution
        && absence.cleanup == proof.cleanup
        && !absence.pidfd_cessation
        && cleanup.ownership == intent.launcher
        && cleanup.container.is_none()
        && matches!(
            cleanup.launcher,
            ProcessStop::Absent | ProcessStop::Reaped { .. }
        )
        && pending.intent == *intent
        && pending.name_exists_exit == 1
        && pending.sweep_exit == 0
        && pending.remaining_ids.is_empty()
        && pending.started_monotonic_ns > exit.monotonic_ns
        && pending.finished_monotonic_ns >= pending.started_monotonic_ns)
}

pub(super) fn candidate(record: &Record, proofs: &[LaunchFailureEvidence]) -> Result<bool> {
    if !matches!(
        record.request.runtime.runtime,
        ExecutionRuntime::RootlessPodman {
            mode: ContainerMode::InJob,
            ..
        }
    ) || record.container_launches.is_empty()
        || !record.runtime_ownerships.is_empty()
        || proofs.len() != record.container_launches.len()
    {
        return Ok(false);
    }
    let mut seen = BTreeSet::new();
    for proof in proofs {
        if !seen.insert(&proof.execution) || !local(record, proof)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn collect_candidates(record: &Record) -> Result<Vec<LaunchFailureEvidence>> {
    let mut proofs = vec![];
    for execution in record.container_launches.keys() {
        let Some(cleanup) = record.cleanup_evidence.get(execution) else {
            return Ok(vec![]);
        };
        let path = record
            .intent
            .output_dir
            .join(format!("{execution}.runtime.jsonl"));
        let raw = read(&path)?;
        let proof = LaunchFailureEvidence {
            execution: execution.clone(),
            process_record: BlobRef {
                path,
                sha256: digest(&raw),
                bytes: raw.len() as u64,
            },
            cleanup: cleanup.clone(),
        };
        proofs.push(proof);
    }
    Ok(if candidate(record, &proofs)? {
        proofs
    } else {
        vec![]
    })
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct JobUnitAbsenceInput<'a> {
    schema: &'static str,
    run: &'a BackendId,
    owner: Owner,
    request_digest: &'a str,
    host: &'a str,
    nonce: &'a str,
    budget_seconds: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationObservation {
    #[serde(rename = "exit_code")]
    _exit_code: i32,
    stdout: String,
    stderr: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum UnitObservation {
    Absent {
        errno: i32,
    },
    Present,
    Unknown {
        #[serde(rename = "errno")]
        _errno: Option<i32>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ControlObservation {
    Directory,
    NotDirectory,
    Absent {
        #[serde(rename = "errno")]
        _errno: i32,
    },
    Unknown {
        #[serde(rename = "errno")]
        _errno: Option<i32>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnitControl {
    path: String,
    observation: ControlObservation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobUnitAbsence {
    schema: String,
    run: BackendId,
    owner: Owner,
    request_digest: String,
    host: String,
    path: String,
    nonce: String,
    generation_before: GenerationObservation,
    control: UnitControl,
    observation: UnitObservation,
    generation_after: GenerationObservation,
    started_monotonic_ns: u64,
    finished_monotonic_ns: u64,
}

const UNIT_CONTROL_PATH: &str =
    "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/flux.slice";

fn unit_path(run: &BackendId) -> Result<String> {
    run.validate()?;
    if canonical_job_id(&run.job)? != run.job {
        return Err(BackendError::invalid("noncanonical job unit"));
    }
    Ok(format!(
        "{UNIT_CONTROL_PATH}/flux-jobs.slice/shell-{}:sys.service",
        run.job
    ))
}

fn absence_matches(record: &Record, nonce: &str, observation: &JobUnitAbsence) -> Result<bool> {
    let Some(binding) = &record.binding else {
        return Ok(false);
    };
    let bounded_observation =
        |facts: &GenerationObservation| facts.stdout.len() <= 16384 && facts.stderr.len() <= 16384;
    let mut last_local_observation = 0;
    for execution in record.container_launches.keys() {
        let Some(reference) = record.cleanup_evidence.get(execution) else {
            return Ok(false);
        };
        let cleanup: CleanupEvidence =
            serde_json::from_slice(&checked_blob(reference, &record.intent.output_dir)?)?;
        let Some(pending) = cleanup.pending_launch else {
            return Ok(false);
        };
        last_local_observation = last_local_observation.max(pending.finished_monotonic_ns);
    }
    Ok(observation.schema == "tl3.job_unit_absence.v1"
        && observation.run == binding.id
        && observation.owner.owner == record.intent.owner
        && observation.owner.correlation == record.intent.correlation
        && observation.request_digest == record.intent.request_digest
        && observation.nonce == nonce
        && observation.host == "dl380g10"
        && record.container_launches.values().all(|intent| {
            intent.launcher.host == observation.host && intent.launcher.run == observation.run
        })
        && observation.path == unit_path(&binding.id)?
        && observation.control.path == UNIT_CONTROL_PATH
        && matches!(
            observation.control.observation,
            ControlObservation::Directory
        )
        && bounded_observation(&observation.generation_before)
        && bounded_observation(&observation.generation_after)
        && matches!(
            observation.observation,
            UnitObservation::Absent { errno: 2 }
        )
        && observation.started_monotonic_ns > last_local_observation
        && observation.finished_monotonic_ns >= observation.started_monotonic_ns
        && observation.finished_monotonic_ns - observation.started_monotonic_ns <= 10_000_000_000)
}

impl Flux {
    /// Called only by external collection of the new candidate, after native Clean.
    pub(super) fn launch_failure_unit_absence(
        &self,
        record: &Record,
    ) -> Result<Option<Vec<BlobRef>>> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        if !record
            .events
            .iter()
            .any(|event| matches!(event.fact, NativeFact::Clean))
        {
            return Ok(None);
        }
        self.instance(record)?;
        validate::trusted_artifacts(&record.request)?;
        let program = record
            .request
            .runtime
            .cleanup
            .as_ref()
            .ok_or_else(|| BackendError::unknown("pinned unit observer unavailable"))?;
        if digest(&read(&program.executable)?) != program.executable_digest {
            return Ok(None);
        }
        let binding = record
            .binding
            .as_ref()
            .ok_or_else(|| BackendError::unknown("unit observation lacks binding"))?;
        let nonce = digest(
            format!(
                "{:?}:{}:{}",
                std::time::SystemTime::now(),
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )
            .as_bytes(),
        );
        let input = serde_json::to_vec(&JobUnitAbsenceInput {
            schema: "tl3.job_unit_absence_input.v1",
            run: &binding.id,
            owner: Owner {
                owner: record.intent.owner.clone(),
                correlation: record.intent.correlation.clone(),
            },
            request_digest: &record.intent.request_digest,
            host: "dl380g10",
            nonce: &nonce,
            budget_seconds: 5,
        })?;
        let out = &record.intent.output_dir;
        let input_ref = blob(
            &out.join(format!("unit-absence-{nonce}.input.json")),
            &input,
        )?;
        let mut command = std::process::Command::new(&program.executable);
        command
            .args(&program.arguments)
            .env_clear()
            .envs(&record.request.environment);
        let reply = self.capture_command(command, Some(input))?;
        let raw = blob(
            &out.join(format!("unit-absence-{nonce}.stdout")),
            &reply.stdout,
        )?;
        let stderr = blob(
            &out.join(format!("unit-absence-{nonce}.stderr")),
            safe(&String::from_utf8_lossy(&reply.stderr))?.as_bytes(),
        )?;
        let status = blob(
            &out.join(format!("unit-absence-{nonce}.status.json")),
            &serde_json::to_vec(&reply.status)?,
        )?;
        let observed: JobUnitAbsence = match serde_json::from_slice(&reply.stdout) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        if !reply.success || !absence_matches(record, &nonce, &observed)? {
            return Ok(None);
        }
        // Recheck local facts and the pinned executable after collecting the fresh observation.
        validate::trusted_artifacts(&record.request)?;
        let latest = self.journal.load(&Owner {
            owner: record.intent.owner.clone(),
            correlation: record.intent.correlation.clone(),
        })?;
        if latest.container_launches != record.container_launches
            || latest.runtime_ownerships != record.runtime_ownerships
            || latest.cleanup_evidence != record.cleanup_evidence
            || latest.cancel_requested != record.cancel_requested
        {
            return Ok(None);
        }
        Ok(Some(vec![input_ref, raw, stderr, status]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires sealed fBELmqkrH5 evidence on /mnt/shared; run through admitted PB"]
    fn retained_fbelmqkrh5_success_keeps_its_checks_verdict_and_evidence() {
        let sealed =
            Path::new("/mnt/shared/flux-out/tl3/fxr-5dd-native-execution-20261107T192713Z");
        let record: Record = serde_json::from_slice(
            &read(&sealed.join("journal-after-cleanup/fxr-sandbox5dd-20261107t191914z/state.json"))
                .unwrap(),
        )
        .unwrap();
        let report_raw = read(&sealed.join("retained-output/report.json")).unwrap();
        let report: RunnerReport = serde_json::from_slice(&report_raw).unwrap();
        assert_eq!(report.run.job, "fBELmqkrH5");
        assert_eq!(report.outcome, ReportOutcome::ChecksCompleted);
        assert!(report.launch_failures.is_empty());
        let journal = tempfile::tempdir().unwrap();
        let backend = Flux::new(Options {
            executable: "/must-not-execute-native".into(),
            arguments: vec![],
            environment: [
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("HOME".into(), "/home/rob".into()),
                ("XDG_RUNTIME_DIR".into(), "/run/user/1000".into()),
            ]
            .into(),
            instance_file: None,
            instance_generation: record.intent.instance.clone(),
            uri: record.intent.uri.clone(),
            journal_root: journal.path().to_owned(),
            bootstrap: HostBootstrap {
                cli: "/must-not-execute-cli".into(),
                config: "/unused-config".into(),
            },
        })
        .unwrap();
        // Calling only identity validation cannot execute the new external mode.
        assert!(
            backend
                .report_identity(&record, record.binding.as_ref().unwrap(), &report)
                .unwrap()
        );
        let payload =
            checked_blob(report.payload.as_ref().unwrap(), &record.intent.output_dir).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert!(
            value.to_string().contains("fail"),
            "the uncorrected red_message fixture verdict must remain"
        );
        assert_eq!(
            digest(&report_raw),
            digest(&read(&sealed.join("retained-output/report.json")).unwrap())
        );
        if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
            std::fs::create_dir_all(&destination).unwrap();
            std::fs::write(Path::new(&destination).join("retained-fBELmqkrH5-replay.json"), serde_json::to_vec_pretty(&serde_json::json!({"report_sha256":digest(&report_raw),"payload_sha256":digest(&payload),"report_accepted":true,"payload":value,"external_absence_called":false})).unwrap()).unwrap();
        }
    }

    #[test]
    #[ignore = "requires sealed fBPZb5i8YP evidence on /mnt/shared; run through admitted PB"]
    fn retained_fbpzb5i8yp_without_launcher_exit_cannot_be_upgraded() {
        let sealed =
            Path::new("/mnt/shared/flux-out/tl3/fxr-canary2-native-execution-20261107T194726Z");
        assert_eq!(
            digest(&read(&sealed.join("STOP-report.json")).unwrap()),
            "fda68c8b4eb08c2a83a7fa4f4ee4d21c85feb732f234e387a31083b6f0a0b688"
        );
        let record: Record = serde_json::from_slice(
            &read(&sealed.join("journal-at-STOP/fxr-canary2-20261107t193732z/state.json")).unwrap(),
        )
        .unwrap();
        let mut report: RunnerReport =
            serde_json::from_slice(&read(&sealed.join("retained-output/report.json")).unwrap())
                .unwrap();
        assert_eq!(report.run.job, "fBPZb5i8YP");
        assert_eq!(report.outcome, ReportOutcome::ChecksCompleted);
        assert!(record.runtime_ownerships.is_empty());
        let mut proofs = vec![];
        let mut journals = vec![];
        for execution in record.container_launches.keys() {
            let filename = format!("{execution}.runtime.jsonl");
            let raw = read(&sealed.join("retained-output").join(&filename)).unwrap();
            let rows: Vec<serde_json::Value> = raw
                .split(|byte| *byte == b'\n')
                .filter(|row| !row.is_empty())
                .map(|row| serde_json::from_slice(row).unwrap())
                .collect();
            assert!(event::<LauncherExit>(&rows, "launcher_exit").is_err());
            assert!(!rows.iter().any(|row| row["event"] == "container"));
            journals.push(serde_json::json!({"file":filename,"sha256":digest(&raw),"launcher_exit":"NOT_RETAINED"}));
            proofs.push(LaunchFailureEvidence {
                execution: execution.clone(),
                process_record: BlobRef {
                    path: record.intent.output_dir.join(filename),
                    bytes: raw.len() as u64,
                    sha256: digest(&raw),
                },
                cleanup: record.cleanup_evidence[execution].clone(),
            });
        }
        assert_eq!(proofs.len(), 2);
        let journal = tempfile::tempdir().unwrap();
        let backend = Flux::new(Options {
            executable: "/must-not-execute-native".into(),
            arguments: vec![],
            environment: [
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("HOME".into(), "/home/rob".into()),
                ("XDG_RUNTIME_DIR".into(), "/run/user/1000".into()),
            ]
            .into(),
            instance_file: None,
            instance_generation: record.intent.instance.clone(),
            uri: record.intent.uri.clone(),
            journal_root: journal.path().to_owned(),
            bootstrap: HostBootstrap {
                cli: "/must-not-execute-cli".into(),
                config: "/unused-config".into(),
            },
        })
        .unwrap();
        assert!(
            !backend
                .report_identity(&record, record.binding.as_ref().unwrap(), &report)
                .unwrap_or(false)
        );
        report.outcome = ReportOutcome::PostBeginLaunchFailure;
        report.complete = false;
        report.cleanup_complete = false;
        report.launch_failures = proofs;
        assert!(
            !backend
                .report_identity(&record, record.binding.as_ref().unwrap(), &report)
                .unwrap_or(false)
        );
        if let Ok(destination) = std::env::var("FX_E2E_EVIDENCE") {
            std::fs::create_dir_all(&destination).unwrap();
            std::fs::write(Path::new(&destination).join("retained-fBPZb5i8YP-replay.json"), serde_json::to_vec_pretty(&serde_json::json!({"sealed_stop_sha256":"fda68c8b4eb08c2a83a7fa4f4ee4d21c85feb732f234e387a31083b6f0a0b688","old_report_rejected":true,"forged_upgrade_rejected":true,"journals":journals})).unwrap()).unwrap();
        }
    }
}
