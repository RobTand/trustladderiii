//! Public Flux 0.91 CLI only. Retained instance generations are operator-owned.
mod cleanup;
mod evidence;
mod process;
mod resources;
mod validate;

use crate::run_backend::{durable::*, *};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;

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
        Ok(Self { options, journal, stop: WatchStop::default() })
    }

    pub fn prepare_intent(&self, request: &Request, owner: &Owner) -> Result<SubmissionIntent> {
        let intent = SubmissionIntent { backend: BackendTag::Flux, instance: self.options.instance_generation.clone(), uri: self.options.uri.clone(), owner: owner.owner.clone(), correlation: owner.correlation.clone(), request_digest: digest(&serde_json::to_vec(request)?), output_dir: Path::new("/mnt/shared/flux-out/tl3").join(&owner.correlation) };
        validate::request(request, &intent, &self.options)?;
        Ok(intent)
    }

    fn instance(&self, record: &Record) -> Result<()> {
        if record.intent.instance != self.options.instance_generation || record.intent.uri != self.options.uri {
            return Err(BackendError::unknown("instance generation changed; no adoption across broker handoff"));
        }
        if let Some(binding) = &record.binding { binding.id.validate()?; }
        Ok(())
    }

    fn recover(&self, owner: &Owner) -> Result<Option<RunBinding>> {
        let record = self.journal.load(owner)?;
        self.instance(&record)?;
        if record.binding.is_some() { return Ok(record.binding); }
        let query = self.capture(&["jobs", "-a", "--json", &format!("--name={}", record.intent.correlation)])?;
        if !query.success { return Ok(None); }
        let jobs: process::Jobs = serde_json::from_slice(&query.stdout)?;
        if jobs.jobs.len() > 128 { return Err(BackendError::unknown("too many recovery candidates")); }
        let mut matches = Vec::new();
        let mut conflicting = false;
        for job in jobs.jobs {
            if job.name != record.intent.correlation { continue; }
            if !f58(&job.jobid) { return Err(BackendError::unknown("invalid recovery job ID")); }
            let original = self.capture(&["job", "info", "--original", &job.jobid, "jobspec"])?;
            if !original.success { return Err(BackendError::unknown("original jobspec unavailable")); }
            let spec: process::Jobspec = serde_json::from_slice(&original.stdout)?;
            if self.matches(&record, &spec)? {
                matches.push(job.jobid);
            } else { conflicting = true; }
        }
        if matches.len() != 1 || conflicting { return Ok(None); }
        let binding = RunBinding {
            id: BackendId { backend: BackendTag::Flux, instance: record.intent.instance.clone(), uri: record.intent.uri.clone(), job: matches.remove(0) },
            intent: record.intent,
        };
        self.journal.transaction(owner, |dir, current| {
            let mut current = current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
            if let Some(existing) = current.binding {
                if existing != binding { return Err(BackendError::unknown("conflicting durable binding")); }
            } else {
                if current.runtime_ownership.as_ref().is_some_and(|owned| owned.run != binding.id) { return Err(BackendError::unknown("recovered binding conflicts with runtime ownership")); }
                if current.runtime_ownership.as_ref().is_some_and(|owned| owned.run != binding.id) { return Err(BackendError::unknown("binding conflicts with retained runtime ownership")); }
                current.binding = Some(binding.clone());
                save(dir, &current)?;
            }
            Ok(())
        })?;
        Ok(Some(binding))
    }

    pub fn validate_runtime_binding(&self, owner: &Owner, facts: &RuntimeBindingInput) -> Result<RuntimeQualification> {
        let record = self.journal.load(owner)?;
        self.instance(&record)?;
        evidence::runtime_binding(&record, facts)
    }

    fn terminal(&self, owner: &Owner, ending: Ending, reason: &str, evidence: Vec<BlobRef>, report: Option<BlobRef>) -> Result<Terminal> {
        self.journal.transaction(owner, |dir, record| {
            let mut record = record.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
            if let Some(terminal) = record.terminal { return Ok(terminal); }
            let receipt = Receipt {
                schema: "tl3.run_receipt.v1".into(), issuer: ReceiptIssuer::Tl3,
                request: blob(&record.intent.output_dir.join("request.json"), &serde_json::to_vec(&record.request)?)?,
                intent: record.intent.clone(), binding: record.binding.clone(),
                tree: record.request.tree.clone(), runner: record.request.runner.clone(), runtime: record.request.runtime.clone(),
                reservation: record.request.reservation.clone(), ending, reason: safe(reason)?, native_events: record.events.clone(), evidence, report, runtime_ownership: record.runtime_ownership.clone(), cleanup: record.cleanup_evidence.clone(),
            };
            let out = &record.intent.output_dir;
            let receipt = blob(&out.join("receipt.json"), &serde_json::to_vec(&receipt)?)?;
            let terminal = Terminal { ending, reason: safe(reason)?, receipt };
            record.terminal = Some(terminal.clone());
            save(dir, &record)?;
            Ok(terminal)
        })
    }
}

impl RunBackend for Flux {
    fn submit(&self, request: Request, intent: SubmissionIntent) -> Result<Submission> {
        validate::request(&request, &intent, &self.options)?;
        let owner = Owner { owner: intent.owner.clone(), correlation: intent.correlation.clone() };
        let (record, fresh) = self.journal.transaction(&owner, |dir, existing| {
            if let Some(record) = existing {
                if record.request != request || record.intent != intent {
                    return Err(BackendError { code: ErrorCode::Conflict, message: "correlation already has another immutable request".into() });
                }
                return Ok((record, false));
            }
            let record = Record { request, intent, dispatched: false, binding: None, cancel_requested: false, events: vec![], terminal: None, runtime_ownership: None, cleanup_evidence: None, jobspec: None };
            save(dir, &record)?;
            Ok((record, true))
        })?;
        self.instance(&record)?;
        if let Some(binding) = record.binding { return Ok(Submission::Accepted { binding }); }
        if let Some(terminal) = record.terminal { return Ok(Submission::Refused { terminal }); }
        if !fresh { return Ok(Submission::Unknown { reason: "intent already retained; attach for reconciliation, never resubmit".into() }); }
        // Reserve output before writing any evidence, including a preflight refusal.
        if std::fs::create_dir(&record.intent.output_dir).is_err() {
            return Ok(Submission::Unknown { reason: "output directory unavailable or already exists; nothing dispatched".into() });
        }
        if let Err(error) = validate::preflight(&record.request) {
            let terminal = self.terminal(&owner, Ending::Refused, &error.message, vec![], None)?;
            return Ok(Submission::Refused { terminal });
        }
        blob(&record.intent.output_dir.join("request.json"), &serde_json::to_vec(&record.request)?)?;
        blob(&record.intent.output_dir.join("intent.json"), &serde_json::to_vec(&record.intent)?)?;
        let mut prepare_args = self.submit_args(&record)?;
        prepare_args.insert(1, "--dry-run".into());
        let prepared = self.capture_owned(&prepare_args)?;
        if !prepared.success { return Ok(Submission::Unknown { reason: "native immutable jobspec preparation failed; nothing submitted; intent retained".into() }); }
        let spec: process::Jobspec = serde_json::from_slice(&prepared.stdout)?;
        let spec = blob(&record.intent.output_dir.join("original-jobspec.json"), &serde_json::to_vec(&spec)?)?;
        self.journal.runtime_gate(|debts| {
        if debts.join(&record.request.runtime.manifest_digest).exists() && std::fs::read_dir(debts.join(&record.request.runtime.manifest_digest))?.next().is_some() { return Err(BackendError::unknown("unresolved runtime cleanup debt; dispatch held")); }
        self.journal.transaction(&owner, |dir, current| {
            let mut current = current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
            if current.cancel_requested { return Err(BackendError::unknown("cancel intent arrived before dispatch; submission held")); }
            current.jobspec = Some(spec);
            current.dispatched = true;
            save(dir, &current)
        })
        })?;
        let args = vec!["job".into(), "submit".into(), record.intent.output_dir.join("original-jobspec.json").to_string_lossy().into_owned()];
        let reply = match self.capture_owned(&args) {
            Ok(reply) => reply,
            Err(_) => return Ok(Submission::Unknown { reason: "submit transport failed after dispatch; reconcile original jobspec".into() }),
        };
        let id = std::str::from_utf8(&reply.stdout).ok().map(str::trim).filter(|id| f58(id));
        let binding = if reply.success && let Some(id) = id {
            let binding = RunBinding { id: BackendId { backend: BackendTag::Flux, instance: record.intent.instance.clone(), uri: record.intent.uri.clone(), job: id.into() }, intent: record.intent.clone() };
            // First durable operation after a successful ID: file sync, rename, directory sync.
            self.journal.transaction(&owner, |dir, current| {
                let mut current = current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
                if current.binding.as_ref().is_some_and(|existing| existing != &binding) {
                    return Err(BackendError::unknown("submit ID conflicts with concurrently recovered binding"));
                }
                if current.runtime_ownership.as_ref().is_some_and(|owned| owned.run != binding.id) { return Err(BackendError::unknown("binding conflicts with retained runtime ownership")); }
                current.binding = Some(binding.clone());
                save(dir, &current)
            })?;
            Some(binding)
        } else { None };
        blob(&record.intent.output_dir.join("submit.stderr"), safe(&String::from_utf8_lossy(&reply.stderr))?.as_bytes())?;
        blob(&record.intent.output_dir.join("submit-status.json"), &serde_json::to_vec(&reply.status)?)?;
        Ok(match binding {
            Some(binding) => Submission::Accepted { binding },
            None => Submission::Unknown { reason: "submit did not establish an ID; nonzero exit is not proof of refusal".into() },
        })
    }

    fn attach(&self, owner: &Owner, stop: &WatchStop, emit: &mut dyn FnMut(&Event) -> Result<()>) -> Result<Observation> {
        let mut observer = self.clone();
        observer.stop = stop.clone();
        observer.attach_owned(owner, stop, emit)
    }

    fn cancel(&self, owner: &Owner) -> Result<CancelAck> {
        let record = self.journal.transaction(owner, |dir, current| {
            let mut current = current.ok_or_else(|| BackendError::invalid("submission intent not found"))?;
            self.instance(&current)?;
            current.cancel_requested = true;
            save(dir, &current)?;
            Ok(current)
        })?;
        if let Some(terminal) = record.terminal { return Ok(CancelAck { binding: record.binding, acknowledged: true, terminal: Some(terminal) }); }
        self.mark_cleanup_debt(&record)?;
        let binding = self.recover(owner).ok().flatten().or(record.binding);
        let acknowledged = if let Some(binding) = &binding {
            self.capture(&["cancel", &binding.id.job]).is_ok_and(|reply| reply.success)
        } else { false };
        let _ = self.run_cleanup(owner);
        Ok(CancelAck { binding, acknowledged, terminal: None })
    }

    fn probe(&self, reservation: &Reservation, placement: &Placement) -> Result<Probe> { self.probe_resources(reservation, placement) }

    fn retain_runtime(&self, owner: &Owner, owned: RuntimeOwnership) -> Result<()> {
        self.journal.transaction(owner, |dir, current| {
            let mut current = current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
            self.instance(&current)?;
            cleanup::ownership(&current, &owned)?;
            if current.cancel_requested || current.terminal.is_some() { return Err(BackendError::unknown("execution held after cancel/terminal")); }
            if current.runtime_ownership.as_ref().is_some_and(|existing| existing != &owned) {
                return Err(BackendError::invalid("runtime ownership is immutable"));
            }
            current.runtime_ownership = Some(owned);
            save(dir, &current)
        })
    }

    fn cleanup_runtime(&self, owner: &Owner) -> Result<CleanupResult> { self.run_cleanup(owner) }

    fn daytime_payload(&self, owner: &Owner, terminal: &Terminal) -> Result<crate::contracts::report::CheckReport> {
        self.verified_daytime_payload(owner, terminal)
    }

    fn launch_context(&self, owner: &Owner, job: &str) -> Result<LaunchContext> {
        let record = self.journal.load(owner)?;
        self.instance(&record)?;
        if !f58(job) || record.binding.as_ref().is_some_and(|binding| binding.id.job != job) || !record.dispatched {
            return Err(BackendError::invalid("native job does not match retained dispatch"));
        }
        Ok(LaunchContext {
            schema: LaunchSchema::V1,
            run: BackendId { backend: BackendTag::Flux, instance: record.intent.instance.clone(), uri: record.intent.uri.clone(), job: job.into() },
            owner: record.intent.owner, correlation: record.intent.correlation, request_digest: record.intent.request_digest,
            scratch: OwnedScratch { approved_root: record.request.scratch_root.clone(), leaf: record.request.scratch_root.join(job) },
            output_dir: record.intent.output_dir.clone(),
            progress: ProgressPolicy { journal: record.intent.output_dir.join("progress.json"), no_progress_seconds: record.request.no_progress_seconds },
            native_threads: record.request.reservation.native_threads, runtime: record.request.runtime.runtime,
            placement: LaunchPlacement::Unqualified { reason: "collect retained native R, topology and host/container facts; validate_runtime_binding before accepting evidence".into() },
        })
    }
}

impl Flux {
    fn attach_owned(&self, owner: &Owner, stop: &WatchStop, emit: &mut dyn FnMut(&Event) -> Result<()>) -> Result<Observation> {
        let record = self.journal.load(owner)?;
        self.instance(&record)?;
        if let Some(terminal) = record.terminal { return Ok(Observation::Terminal { binding: record.binding, terminal }); }
        let binding = match self.recover(owner) {
            Ok(Some(binding)) => binding,
            Ok(None) => return Ok(Observation::Unknown { binding: None, reason: "zero, conflicting, or multiple original-jobspec matches; no resubmission".into() }),
            Err(_) => return Ok(Observation::Unknown { binding: record.binding, reason: "recovery transport or identity unavailable".into() }),
        };
        if self.journal.load(owner)?.cancel_requested { let _ = self.cancel(owner); }
        use std::os::unix::process::CommandExt;
        let original = self.capture(&["job", "info", "--original", &binding.id.job, "jobspec"])?;
        if !original.success || !self.matches(&self.journal.load(owner)?, &serde_json::from_slice(&original.stdout)?)? {
            return Ok(Observation::Unknown { binding: Some(binding), reason: "saved ID original jobspec unavailable or mismatched".into() });
        }
        let child = self.command().args(["job", "eventlog", "--follow", "--format=json", &binding.id.job]).process_group(0)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let mut observer = process::Observer::new(child, stop.clone());
        let stderr = observer.child.stderr.take().ok_or_else(|| BackendError::unknown("watch stderr unavailable"))?;
        let stderr = std::thread::spawn(move || process::bounded(stderr));
        let stdout = observer.child.stdout.take().ok_or_else(|| BackendError::unknown("watch stdout unavailable"))?;
        let mut reader = BufReader::new(stdout);
        let mut position = 0_usize;
        let mut clean = false;
        let watched = (|| -> Result<()> {
            loop {
                let mut bytes = Vec::new();
                reader.by_ref().take(65_537).read_until(b'\n', &mut bytes)?;
                if bytes.is_empty() { break; }
                if bytes.len() > 65_536 { return Err(BackendError::unknown("oversized native event")); }
                let fact = process::parse_event(&bytes)?;
                let event_digest = digest(&bytes);
                let event = self.journal.transaction(owner, |dir, current| {
                    let mut current = current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
                    if current.terminal.is_some() { return Ok(None); }
                    if let Some(previous) = current.events.get(position) {
                        if previous.digest != event_digest { return Err(BackendError::unknown("retained history changed or was purged")); }
                        return Ok(None);
                    }
                    if position != current.events.len() || position >= 16_384 { return Err(BackendError::unknown("invalid replay sequence or event limit")); }
                    let event = Event { sequence: position as u64, digest: event_digest, fact: fact.clone() };
                    current.events.push(event.clone());
                    save(dir, &current)?;
                    Ok(Some(event))
                })?;
                position += 1;
                if let Some(event) = event { emit(&event)?; }
                if matches!(fact, NativeFact::Clean) { clean = true; break; }
            }
            Ok(())
        })();
        drop(observer);
        let stderr = stderr.join().ok().and_then(std::result::Result::ok).unwrap_or_default();
        let out = &binding.intent.output_dir;
        blob(&out.join("watch.stderr"), safe(&String::from_utf8_lossy(&stderr))?.as_bytes())?;
        if watched.is_err() || !clean || stop.is_stopped() {
            return Ok(Observation::Unknown { binding: Some(binding), reason: "watcher transport/history unavailable; retain binding and reattach".into() });
        }
        self.collect(owner, binding)
    }

}
