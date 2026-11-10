//! Public Flux 0.91 CLI only. Retained instance generations are operator-owned.
mod cleanup;
mod evidence;
mod launch_failure;
mod lifecycle;
mod placement;
mod process;
mod progress;
mod resources;
mod scratch;
mod validate;
mod worker_protocol;

use crate::run_backend::{durable::*, *};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Offline validation only. Native instance generation remains guarded at execution.
pub(crate) fn validate_prepared_runtime(prepared: &PreparedRuntime) -> Result<()> {
    validate::runtime_preflight(&prepared.runtime)
}

#[derive(Clone)]
pub struct Flux {
    options: Options,
    journal: Journal,
    stop: WatchStop,
}

impl Flux {
    pub fn new(options: Options) -> Result<Self> {
        validate::options(&options)?;
        let journal = Journal::new(options.journal_root.clone())?;
        Ok(Self {
            options,
            journal,
            stop: WatchStop::default(),
        })
    }

    pub fn prepare_intent(&self, request: &Request, owner: &Owner) -> Result<SubmissionIntent> {
        let intent = SubmissionIntent {
            backend: BackendTag::Flux,
            instance: if self.options.instance_file.is_some() {
                "runtime".into()
            } else {
                self.options.instance_generation.clone()
            },
            uri: self.options.resolved_uri()?,
            owner: owner.owner.clone(),
            correlation: owner.correlation.clone(),
            request_digest: digest(&serde_json::to_vec(request)?),
            output_dir: Path::new("/mnt/shared/flux-out/tl3").join(&owner.correlation),
        };
        validate::request(request, &intent, &self.options)?;
        Ok(intent)
    }

    pub fn cancel_with_cleanup_progress(
        &self,
        owner: &Owner,
        emit: &mut dyn FnMut(&CleanupProgress) -> Result<()>,
    ) -> Result<CancelAck> {
        let record = self.journal.transaction(owner, |dir, current| {
            let mut current =
                current.ok_or_else(|| BackendError::invalid("submission intent not found"))?;
            self.instance(&current)?;
            current.cancel_requested = true;
            save(dir, &current)?;
            Ok(current)
        })?;
        if let Some(terminal) = record.terminal {
            return Ok(CancelAck {
                binding: record.binding,
                acknowledged: true,
                terminal: Some(terminal),
            });
        }
        if !record.dispatched {
            // Also retire exact unregistered debt retained by earlier versions.
            self.run_cleanup(owner)?;
            let terminal = self.terminal(
                owner,
                Ending::Cancelled,
                "cancelled before durable dispatch; no native or runtime effect",
                vec![],
                None,
            )?;
            return Ok(CancelAck {
                binding: None,
                acknowledged: true,
                terminal: Some(terminal),
            });
        }
        self.mark_cleanup_debt(&record)?;
        let binding = self.recover(owner).ok().flatten().or(record.binding);
        let acknowledged = if let Some(binding) = &binding {
            self.capture(&["cancel", &binding.id.job])
                .is_ok_and(|reply| reply.success)
        } else {
            false
        };
        let mut transport_error = None;
        let _ = self.cleanup_runtime_with_progress(owner, &mut |progress| {
            emit(progress).inspect_err(|error| transport_error = Some(error.clone()))
        });
        if let Some(error) = transport_error {
            return Err(error);
        }
        Ok(CancelAck {
            binding,
            acknowledged,
            terminal: None,
        })
    }

    fn instance(&self, record: &Record) -> Result<()> {
        if let Some(binding) = &record.binding {
            binding.id.validate()?;
            if record.terminal.is_some() {
                return Ok(());
            }
            let original =
                self.capture(&["job", "info", "--original", &binding.id.job, "jobspec"])?;
            if !original.success
                || !self.matches(record, &serde_json::from_slice(&original.stdout)?)?
            {
                return Err(BackendError::unknown(
                    "retained job original jobspec unavailable or mismatched; no adoption",
                ));
            }
        }
        Ok(())
    }

    fn recover(&self, owner: &Owner) -> Result<Option<RunBinding>> {
        let record = self.journal.load(owner)?;
        self.instance(&record)?;
        if record.binding.is_some() {
            return Ok(record.binding);
        }
        let query = self.capture(&[
            "jobs",
            "-a",
            "--json",
            &format!("--name={}", record.intent.correlation),
        ])?;
        if !query.success {
            return Ok(None);
        }
        let jobs: process::Jobs = serde_json::from_slice(&query.stdout)?;
        if jobs.jobs.len() > 128 {
            return Err(BackendError::unknown("too many recovery candidates"));
        }
        let mut matches = Vec::new();
        let mut conflicting = false;
        for job in jobs.jobs {
            if job.name != record.intent.correlation {
                continue;
            }
            let original = self.capture(&["job", "info", "--original", &job.jobid, "jobspec"])?;
            if !original.success {
                return Err(BackendError::unknown("original jobspec unavailable"));
            }
            let spec: process::Jobspec = serde_json::from_slice(&original.stdout)?;
            if self.matches(&record, &spec)? {
                matches.push(job.jobid);
            } else {
                conflicting = true;
            }
        }
        if matches.len() != 1 || conflicting {
            return Ok(None);
        }
        let binding = RunBinding {
            id: BackendId {
                backend: BackendTag::Flux,
                instance: record.intent.instance.clone(),
                uri: record.intent.uri.clone(),
                job: matches.remove(0),
            },
            intent: record.intent,
        };
        self.journal.transaction(owner, |dir, current| {
            let mut current = current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
            if let Some(existing) = current.binding {
                if existing != binding {
                    return Err(BackendError::unknown("conflicting durable binding"));
                }
            } else {
                if current
                    .runtime_ownerships
                    .iter()
                    .any(|owned| owned.run != binding.id)
                {
                    return Err(BackendError::unknown(
                        "binding conflicts with retained runtime ownership",
                    ));
                }
                current.binding = Some(binding.clone());
                save(dir, &current)?;
            }
            Ok(())
        })?;
        Ok(Some(binding))
    }

    pub fn validate_runtime_binding(
        &self,
        owner: &Owner,
        facts: &RuntimeBindingInput,
    ) -> Result<RuntimeQualification> {
        let record = self.journal.load(owner)?;
        self.instance(&record)?;
        if record.cancel_requested || record.terminal.is_some() {
            return Err(BackendError::unknown(
                "payload gate is held after cancel or terminal",
            ));
        }
        evidence::runtime_binding(&record, facts)
    }

    fn terminal(
        &self,
        owner: &Owner,
        ending: Ending,
        reason: &str,
        evidence: Vec<BlobRef>,
        report: Option<BlobRef>,
    ) -> Result<Terminal> {
        self.journal.transaction(owner, |dir, record| {
            let mut record = record.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
            if let Some(terminal) = record.terminal {
                return Ok(terminal);
            }
            if ending != Ending::Refused || record.dispatched {
                if !cleanup::all_stopped(&record) {
                    return Err(BackendError::unknown(
                        "new execution or incomplete cleanup prevents terminal commit",
                    ));
                }
                if let Some(reference) = &report {
                    let retained: RunnerReport = serde_json::from_slice(&checked_blob(
                        reference,
                        &record.intent.output_dir,
                    )?)?;
                    let binding = record
                        .binding
                        .as_ref()
                        .ok_or_else(|| BackendError::unknown("terminal binding unavailable"))?;
                    if !self.report_identity(&record, binding, &retained)? {
                        return Err(BackendError::unknown(
                            "report does not cover current retained executions",
                        ));
                    }
                }
            }
            let receipt = Receipt {
                schema: "tl3.run_receipt.v1".into(),
                issuer: ReceiptIssuer::Tl3,
                request: blob(
                    &record.intent.output_dir.join("request.json"),
                    &serde_json::to_vec(&record.request)?,
                )?,
                intent: record.intent.clone(),
                binding: record.binding.clone(),
                tree: record.request.tree.clone(),
                runner: record.request.runner.clone(),
                runtime: record.request.runtime.clone(),
                reservation: record.request.reservation.clone(),
                ending,
                reason: safe(reason)?,
                native_events: record.events.clone(),
                evidence,
                report,
                runtime_ownerships: record.runtime_ownerships.clone(),
                container_launches: record.container_launches.values().cloned().collect(),
                cleanup: record.cleanup_evidence.clone(),
            };
            let out = &record.intent.output_dir;
            let receipt = blob(&out.join("receipt.json"), &serde_json::to_vec(&receipt)?)?;
            let terminal = Terminal {
                ending,
                reason: safe(reason)?,
                receipt,
            };
            record.terminal = Some(terminal.clone());
            save(dir, &record)?;
            Ok(terminal)
        })
    }
}

impl RunBackend for Flux {
    fn begin_container_launch(
        &self,
        owner: &Owner,
        launcher: &RuntimeOwnership,
    ) -> Result<ContainerLaunchIntent> {
        self.begin_launch(owner, launcher)
    }
    fn bind_container_launch(
        &self,
        owner: &Owner,
        observed: RuntimeOwnership,
    ) -> Result<ContainerLaunchBinding> {
        self.bind_launch(owner, observed)
    }
    fn submit(&self, request: Request, intent: SubmissionIntent) -> Result<Submission> {
        validate::request(&request, &intent, &self.options)?;
        let owner = Owner {
            owner: intent.owner.clone(),
            correlation: intent.correlation.clone(),
        };
        let (record, fresh) = self.journal.transaction(&owner, |dir, existing| {
            if let Some(record) = existing {
                if record.request != request || record.intent != intent {
                    return Err(BackendError {
                        code: ErrorCode::Conflict,
                        message: "correlation already has another immutable request".into(),
                    });
                }
                return Ok((record, false));
            }
            let record = Record {
                request,
                intent,
                dispatched: false,
                binding: None,
                cancel_requested: false,
                events: vec![],
                terminal: None,
                runtime_ownerships: vec![],
                container_launches: Default::default(),
                cleanup_evidence: Default::default(),
                jobspec: None,
                allocation_grant: None,
                instance_observation: None,
            };
            save(dir, &record)?;
            Ok((record, true))
        })?;
        self.instance(&record)?;
        if let Some(binding) = record.binding {
            return Ok(Submission::Accepted { binding });
        }
        if let Some(terminal) = record.terminal {
            return Ok(Submission::Refused { terminal });
        }
        if !fresh {
            return Ok(Submission::Unknown {
                reason: "intent already retained; attach for reconciliation, never resubmit".into(),
            });
        }
        // Reserve output before writing any evidence, including a preflight refusal.
        if std::fs::create_dir(&record.intent.output_dir).is_err() {
            return Ok(Submission::Unknown {
                reason: "output directory unavailable or already exists; nothing dispatched".into(),
            });
        }
        if let Err(error) = validate::preflight(&record.request) {
            let terminal = self.terminal(&owner, Ending::Refused, &error.message, vec![], None)?;
            return Ok(Submission::Refused { terminal });
        }
        blob(
            &record.intent.output_dir.join("request.json"),
            &serde_json::to_vec(&record.request)?,
        )?;
        blob(
            &record.intent.output_dir.join("intent.json"),
            &serde_json::to_vec(&record.intent)?,
        )?;
        let start = self.capture(&["getattr", "broker.starttime"]);
        let config = self.capture(&["config", "get"]);
        let observation = InstanceObservation {
            broker_starttime: start
                .ok()
                .filter(|reply| reply.success)
                .and_then(|reply| String::from_utf8(reply.stdout).ok())
                .map(|text| text.trim().to_owned()),
            loaded_config: config
                .ok()
                .filter(|reply| reply.success)
                .map(|reply| {
                    blob(
                        &record.intent.output_dir.join("observed-loaded-config.json"),
                        &reply.stdout,
                    )
                })
                .transpose()?,
            config_sha256: None,
        };
        let observation = InstanceObservation {
            config_sha256: observation
                .loaded_config
                .as_ref()
                .map(|blob| blob.sha256.clone()),
            ..observation
        };
        self.journal.transaction(&owner, |dir, current| {
            let mut current = current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
            current.instance_observation = Some(observation);
            save(dir, &current)
        })?;
        let mut prepare_args = self.submit_args(&record)?;
        prepare_args.insert(1, "--dry-run".into());
        let prepared = self.capture_owned(&prepare_args)?;
        if !prepared.success {
            return Ok(Submission::Unknown { reason: "native immutable jobspec preparation failed; nothing submitted; intent retained".into() });
        }
        let spec = serde_json::from_slice::<process::Jobspec>(&prepared.stdout)?
            .with_memory(&record.request.reservation)?;
        let spec = blob(
            &record.intent.output_dir.join("original-jobspec.json"),
            &serde_json::to_vec(&spec)?,
        )?;
        validate::trusted_artifacts(&record.request)?;
        self.journal.runtime_gate(|debts| {
            if debts.join(&record.request.runtime.manifest_digest).exists()
                && std::fs::read_dir(debts.join(&record.request.runtime.manifest_digest))?
                    .next()
                    .is_some()
            {
                return Err(BackendError::unknown(
                    "unresolved runtime cleanup debt; dispatch held",
                ));
            }
            self.journal.transaction(&owner, |dir, current| {
                let mut current =
                    current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
                if current.cancel_requested {
                    return Err(BackendError::unknown(
                        "cancel intent arrived before dispatch; submission held",
                    ));
                }
                current.jobspec = Some(spec);
                current.dispatched = true;
                save(dir, &current)
            })
        })?;
        let args = vec![
            "job".into(),
            "submit".into(),
            record
                .intent
                .output_dir
                .join("original-jobspec.json")
                .to_string_lossy()
                .into_owned(),
        ];
        let reply = match self.capture_owned(&args) {
            Ok(reply) => reply,
            Err(_) => {
                return Ok(Submission::Unknown {
                    reason: "submit transport failed after dispatch; reconcile original jobspec"
                        .into(),
                });
            }
        };
        let id = std::str::from_utf8(&reply.stdout)
            .ok()
            .map(str::trim)
            .and_then(|id| canonical_job_id(id).ok());
        let binding = if reply.success
            && let Some(id) = id
        {
            let binding = RunBinding {
                id: BackendId {
                    backend: BackendTag::Flux,
                    instance: record.intent.instance.clone(),
                    uri: record.intent.uri.clone(),
                    job: id,
                },
                intent: record.intent.clone(),
            };
            // First durable operation after a successful ID: file sync, rename, directory sync.
            self.journal.transaction(&owner, |dir, current| {
                let mut current =
                    current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
                if current
                    .binding
                    .as_ref()
                    .is_some_and(|existing| existing != &binding)
                {
                    return Err(BackendError::unknown(
                        "submit ID conflicts with concurrently recovered binding",
                    ));
                }
                if current
                    .runtime_ownerships
                    .iter()
                    .any(|owned| owned.run != binding.id)
                {
                    return Err(BackendError::unknown(
                        "binding conflicts with retained runtime ownership",
                    ));
                }
                current.binding = Some(binding.clone());
                save(dir, &current)
            })?;
            Some(binding)
        } else {
            None
        };
        blob(
            &record.intent.output_dir.join("submit.stderr"),
            safe(&String::from_utf8_lossy(&reply.stderr))?.as_bytes(),
        )?;
        blob(
            &record.intent.output_dir.join("submit-status.json"),
            &serde_json::to_vec(&reply.status)?,
        )?;
        Ok(match binding {
            Some(binding) => Submission::Accepted { binding },
            None => Submission::Unknown {
                reason: "submit did not establish an ID; nonzero exit is not proof of refusal"
                    .into(),
            },
        })
    }

    fn attach(
        &self,
        owner: &Owner,
        stop: &WatchStop,
        emit: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<Observation> {
        let mut observer = self.clone();
        observer.stop = stop.clone();
        observer.attach_owned(owner, stop, emit)
    }

    fn cancel(&self, owner: &Owner) -> Result<CancelAck> {
        self.cancel_with_cleanup_progress(owner, &mut |_| Ok(()))
    }

    fn liveness(&self, owner: &Owner) -> Result<Liveness> {
        self.observe_liveness(owner)
    }

    fn probe(&self, reservation: &Reservation, placement: &Placement) -> Result<Probe> {
        self.probe_resources(reservation, placement)
    }

    fn retain_runtime(&self, owner: &Owner, owned: RuntimeOwnership) -> Result<()> {
        if matches!(
            self.journal.load(owner)?.request.runtime.runtime,
            ExecutionRuntime::RootlessPodman { .. }
        ) {
            let bound = self.bind_launch(owner, owned)?;
            return if bound.payload_allowed {
                Ok(())
            } else {
                Err(BackendError::unknown(
                    "binding retained for cleanup after cancellation; payload held",
                ))
            };
        }
        self.journal.transaction(owner, |dir, current| {
            let mut current = current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
            self.instance(&current)?;
            cleanup::ownership(&current, &owned)?;
            if let Some(existing) = current
                .runtime_ownerships
                .iter()
                .find(|existing| existing.execution == owned.execution)
            {
                return if existing == &owned {
                    Ok(())
                } else {
                    Err(BackendError::invalid("execution ownership is immutable"))
                };
            }
            if current.cancel_requested || current.terminal.is_some() {
                return Err(BackendError::unknown(
                    "execution held after cancel/terminal",
                ));
            }
            if current.runtime_ownerships.len() >= 1024
                || owned.container.as_ref().is_some_and(|container| {
                    current.runtime_ownerships.iter().any(|existing| {
                        existing
                            .container
                            .as_ref()
                            .is_some_and(|existing| existing.id == container.id)
                    })
                })
            {
                return Err(BackendError::invalid(
                    "duplicate container ownership or registration limit exceeded",
                ));
            }
            current.runtime_ownerships.push(owned);
            save(dir, &current)
        })
    }

    fn validate_allocation(&self, owner: &Owner, facts: &AllocationInput) -> Result<CpuGrant> {
        let record = self.journal.load(owner)?;
        self.instance(&record)?;
        self.check_live_allocation(&record, facts)?;
        let grant = evidence::allocation_grant(&record, facts)?;
        self.journal.transaction(owner, |dir, current| {
            let mut current = current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
            if current.cancel_requested
                || current.terminal.is_some()
                || current
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.id != grant.run)
                || current
                    .allocation_grant
                    .as_ref()
                    .is_some_and(|previous| previous != &grant)
            {
                return Err(BackendError::unknown(
                    "allocation changed or execution held",
                ));
            }
            current.allocation_grant = Some(grant.clone());
            save(dir, &current)
        })?;
        Ok(grant)
    }

    fn cleanup_runtime(&self, owner: &Owner) -> Result<CleanupResult> {
        self.run_cleanup(owner)
    }

    fn worker_payload(&self, owner: &Owner, terminal: &Terminal) -> Result<WorkerPayload> {
        self.verified_worker_payload(owner, terminal)
    }

    fn daytime_payload(
        &self,
        owner: &Owner,
        terminal: &Terminal,
    ) -> Result<crate::contracts::report::CheckReport> {
        self.verified_daytime_payload(owner, terminal)
    }

    fn launch_context(&self, owner: &Owner, job: &str) -> Result<LaunchContext> {
        let record = self.journal.load(owner)?;
        self.instance(&record)?;
        validate::trusted_artifacts(&record.request)?;
        let job = canonical_job_id(job)?;
        if record
            .binding
            .as_ref()
            .is_some_and(|binding| binding.id.job != job)
            || !record.dispatched
        {
            return Err(BackendError::invalid(
                "native job does not match retained dispatch",
            ));
        }
        let output_bounds = output_bounds(&record.request.suite);
        let coordinator_uri = if record.request.pass_flux_uri {
            let reference = record
                .jobspec
                .as_ref()
                .ok_or_else(|| BackendError::unknown("retained jobspec missing"))?;
            let spec: serde_json::Value =
                serde_json::from_slice(&checked_blob(reference, &record.intent.output_dir)?)?;
            Some(
                spec["attributes"]["system"]["environment"]["FLUX_URI"]
                    .as_str()
                    .ok_or_else(|| BackendError::unknown("controller-owned FLUX_URI missing"))?
                    .to_owned(),
            )
        } else {
            None
        };
        Ok(LaunchContext {
            coordinator_uri,
            output_bounds,
            schema: LaunchSchema::V1,
            run: BackendId { backend: BackendTag::Flux, instance: record.intent.instance.clone(), uri: record.intent.uri.clone(), job: job.clone() },
            owner: record.intent.owner, correlation: record.intent.correlation, request_digest: record.intent.request_digest,
            scratch: OwnedScratch { approved_root: record.request.scratch_root.clone(), leaf: record.request.scratch_root.join(&job) },
            output_dir: record.intent.output_dir.clone(),
            progress: ProgressPolicy { journal: record.intent.output_dir.join("progress.json"), no_progress_seconds: record.request.no_progress_seconds },
            native_threads: record.request.reservation.native_threads, runtime: record.request.runtime.runtime.clone(), runtime_identity: record.request.runtime,
            placement: match record.allocation_grant {
                Some(grant) if grant.run.job == job => LaunchPlacement::Observed { allocation: grant.allocation, topology: grant.topology, host: grant.host, rank: grant.rank, os_pus: grant.os_pus },
                _ => LaunchPlacement::Unqualified { reason: "collect retained native R/topology/host facts and call validate_allocation before launch".into() },
            },
        })
    }
}

impl Flux {
    fn attach_owned(
        &self,
        owner: &Owner,
        stop: &WatchStop,
        emit: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<Observation> {
        let record = self.journal.load(owner)?;
        self.instance(&record)?;
        if let Some(terminal) = record.terminal {
            self.retire_scratch_debt(owner, &terminal)?;
            if terminal.ending == Ending::InfraLost {
                self.retire_launch_failure_debt(owner, &terminal)?;
            }
            return Ok(Observation::Terminal {
                binding: record.binding,
                terminal,
            });
        }
        let binding =
            match self.recover(owner) {
                Ok(Some(binding)) => binding,
                Ok(None) => return Ok(Observation::Unknown {
                    binding: None,
                    reason:
                        "zero, conflicting, or multiple original-jobspec matches; no resubmission"
                            .into(),
                }),
                Err(_) => {
                    return Ok(Observation::Unknown {
                        binding: record.binding,
                        reason: "recovery transport or identity unavailable".into(),
                    });
                }
            };
        if self.journal.load(owner)?.cancel_requested {
            let _ = self.cancel(owner);
        }
        use std::os::unix::process::CommandExt;
        let original = self.capture(&["job", "info", "--original", &binding.id.job, "jobspec"])?;
        if !original.success
            || !self.matches(
                &self.journal.load(owner)?,
                &serde_json::from_slice(&original.stdout)?,
            )?
        {
            return Ok(Observation::Unknown {
                binding: Some(binding),
                reason: "saved ID original jobspec unavailable or mismatched".into(),
            });
        }
        let child = self
            .command()?
            .args([
                "job",
                "eventlog",
                "--follow",
                "--format=json",
                &binding.id.job,
            ])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut observer = process::Observer::new(child, stop.clone());
        let stderr = observer
            .child
            .stderr
            .take()
            .ok_or_else(|| BackendError::unknown("watch stderr unavailable"))?;
        let stderr = std::thread::spawn(move || process::bounded(stderr));
        let stdout = observer
            .child
            .stdout
            .take()
            .ok_or_else(|| BackendError::unknown("watch stdout unavailable"))?;
        let mut reader = BufReader::new(stdout);
        let mut position = 0_usize;
        let mut clean = false;
        let watched = (|| -> Result<()> {
            loop {
                let mut bytes = Vec::new();
                reader.by_ref().take(65_537).read_until(b'\n', &mut bytes)?;
                if bytes.is_empty() {
                    break;
                }
                if bytes.len() > 65_536 {
                    return Err(BackendError::unknown("oversized native event"));
                }
                let fact = process::parse_event(&bytes)?;
                let event_digest = digest(&bytes);
                let event = self.journal.transaction(owner, |dir, current| {
                    let mut current =
                        current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
                    if current.terminal.is_some() {
                        return Ok(None);
                    }
                    if let Some(previous) = current.events.get(position) {
                        if previous.digest != event_digest {
                            return Err(BackendError::unknown(
                                "retained history changed or was purged",
                            ));
                        }
                        return Ok(None);
                    }
                    if position != current.events.len() || position >= 16_384 {
                        return Err(BackendError::unknown(
                            "invalid replay sequence or event limit",
                        ));
                    }
                    let event = Event {
                        sequence: position as u64,
                        digest: event_digest.clone(),
                        fact: fact.clone(),
                        progress: NativeProgress {
                            owner: owner.clone(),
                            run: binding.id.clone(),
                            cursor: EventCursor {
                                sequence: position as u64,
                                digest: event_digest,
                            },
                            phase: progress::historical_phase(&fact),
                            authority: ProgressAuthority::Historical,
                        },
                    };
                    current.events.push(event.clone());
                    save(dir, &current)?;
                    Ok(Some(event))
                })?;
                position += 1;
                if let Some(event) = event {
                    emit(&event)?;
                }
                if matches!(fact, NativeFact::Clean) {
                    clean = true;
                    break;
                }
            }
            Ok(())
        })();
        drop(observer);
        let stderr = stderr
            .join()
            .ok()
            .and_then(std::result::Result::ok)
            .unwrap_or_default();
        let out = &binding.intent.output_dir;
        blob(
            &out.join("watch.stderr"),
            safe(&String::from_utf8_lossy(&stderr))?.as_bytes(),
        )?;
        if watched.is_err() || !clean || stop.is_stopped() {
            return Ok(Observation::Unknown {
                binding: Some(binding),
                reason: "watcher transport/history unavailable; retain binding and reattach".into(),
            });
        }
        self.collect(owner, binding)
    }
}
