use super::*;
use std::collections::BTreeSet;

fn unqualified(reason: &str) -> RuntimeQualification {
    RuntimeQualification { qualified: false, reason: reason.into(), evidence: vec![] }
}

pub(super) fn runtime_binding(record: &Record, facts: &RuntimeBindingInput) -> Result<RuntimeQualification> {
    let Some(binding) = &record.binding else { return Ok(unqualified("backend ID not yet durably bound")); };
    if facts.run != binding.id || facts.request_digest != record.intent.request_digest {
        return Ok(unqualified("runtime facts do not match retained backend identity"));
    }
    let checked = (|| -> Result<()> {
        let r: resources::ResourceSet = serde_json::from_slice(&checked_blob(&facts.allocation, &record.intent.output_dir)?)?;
        let topology: TopologyMapping = serde_json::from_slice(&checked_blob(&facts.topology, &record.intent.output_dir)?)?;
        if r.version != 1 || topology.run != binding.id || topology.request_digest != record.intent.request_digest
            || topology.ranks.len() > 4096 || facts.host.host.len() > 256 || !token(&facts.host.host) {
            return Err(BackendError::invalid("runtime allocation/topology identity mismatch"));
        }
        // This first runtime is one native task on one rank, never an inferred cross-rank mask.
        let allocated = r.execution.ranks.iter().filter_map(|rank| {
            resources::idset(&rank.rank).ok().filter(|ranks| ranks.contains(&facts.host.rank)).map(|_| rank)
        }).collect::<Vec<_>>();
        if allocated.len() != 1 { return Err(BackendError::invalid("native rank allocation missing or ambiguous")); }
        let mappings = topology.ranks.iter().filter(|rank| rank.rank == facts.host.rank && rank.host == facts.host.host).collect::<Vec<_>>();
        if mappings.len() != 1 { return Err(BackendError::invalid("native core to OS-PU mapping missing or ambiguous")); }
        let cores = resources::idset(&allocated[0].children.core)?;
        if cores.len() as u64 != u64::from(record.request.reservation.cpus.get()) {
            return Err(BackendError::invalid("observed core grant differs from declared task shape"));
        }
        let mut pus = BTreeSet::new();
        let mut mapped = BTreeSet::new();
        for core in &mappings[0].cores {
            if cores.contains(&core.flux_core) {
                if !mapped.insert(core.flux_core) { return Err(BackendError::invalid("duplicate native core mapping")); }
                let core_pus = resources::idset(&core.os_pus)?;
                if core_pus.is_empty() || core_pus.iter().any(|pu| !pus.insert(*pu)) { return Err(BackendError::invalid("invalid overlapping OS-PU mapping")); }
            }
        }
        if mapped != cores || resources::idset(&facts.host.allowed_cpus)? != pus {
            return Err(BackendError::invalid("host CPU affinity does not match mapped native grant"));
        }
        let cgroup = cgroup_path(&facts.host.cgroup)?;
        if let ExecutionRuntime::RootlessPodman { mode, memory_limit_bytes, .. } = record.request.runtime.runtime {
            let container = facts.container.as_ref().ok_or_else(|| BackendError::invalid("inside-container affinity/cgroup evidence missing"))?;
            if container.rank != facts.host.rank || container.host != facts.host.host || resources::idset(&container.allowed_cpus)? != pus {
                return Err(BackendError::invalid("container CPU affinity differs from native grant"));
            }
            let child = cgroup_path(&container.cgroup)?;
            match mode {
                ContainerMode::InJob => {
                    if child != cgroup && !child.strip_prefix(&cgroup).is_some_and(|suffix| suffix.starts_with('/')) {
                        return Err(BackendError::invalid("in_job container escaped host cgroup"));
                    }
                }
                ContainerMode::OwnScope => {
                    let scope = facts.container_scope.as_ref().ok_or_else(|| BackendError::invalid("own_scope unit/cpuset/memory evidence missing"))?;
                    let prefix = format!("tl3-{}-{}", record.intent.correlation, binding.id.job);
                    if !scope.unit.starts_with(&prefix) || !scope.unit.ends_with(".scope") || !token(&scope.unit)
                        || !hex(&scope.invocation_id, 32) || cgroup_path(&scope.cgroup)? != child || child == cgroup
                        || resources::idset(&scope.cpuset_effective)? != pus
                        || scope.memory_max != memory_limit_bytes.map(|bytes| bytes.get()) || scope.memory_swap_max != Some(0) {
                        return Err(BackendError::invalid("own_scope generation/cpuset/per-container memory cap mismatch"));
                    }
                }
            }
        }
        Ok(())
    })();
    match checked {
        Ok(()) => Ok(RuntimeQualification { qualified: true, reason: "supplied job-bound affinity/cgroup facts agree; live runtime qualification remains separate".into(), evidence: vec![facts.allocation.clone(), facts.topology.clone()] }),
        Err(error) => Ok(unqualified(&error.message)),
    }
}

fn cgroup_path(raw: &str) -> Result<String> {
    let mut lines = raw.lines();
    let line = lines.next().ok_or_else(|| BackendError::invalid("cgroup facts missing"))?;
    let path = line.strip_prefix("0::").ok_or_else(|| BackendError::invalid("unsupported non-unified cgroup facts"))?;
    if lines.next().is_some() || path == "/" || !absolute(Path::new(path)) || path.len() > 4096 {
        return Err(BackendError::invalid("unqualified job cgroup path"));
    }
    Ok(path.into())
}

fn sanitized_json(value: &mut serde_json::Value, depth: usize) -> Result<()> {
    if depth > 32 { return Err(BackendError::invalid("evidence nesting exceeds bound")); }
    match value {
        serde_json::Value::String(text) => *text = safe(text)?,
        serde_json::Value::Array(values) => {
            if values.len() > 100_000 { return Err(BackendError::invalid("evidence collection exceeds bound")); }
            for value in values { sanitized_json(value, depth + 1)?; }
        }
        serde_json::Value::Object(values) => {
            if values.len() > 256 { return Err(BackendError::invalid("evidence fields exceed bound")); }
            for (name, value) in values {
                if name.len() > 256 || safe(name)? != *name { return Err(BackendError::invalid("unsafe evidence field name")); }
                sanitized_json(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn retain_json(out: &Path, name: &str, bytes: &[u8]) -> Result<BlobRef> {
    let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
    sanitized_json(&mut value, 0)?;
    blob(&out.join(name), &serde_json::to_vec(&value)?)
}

impl Flux {
    /// Convert only the trusted host stdout protocol. Existing daytime judging remains in FX2.
    pub fn collect_runner(&self, owner: &Owner, input: &CollectorInput) -> Result<(BlobRef, BlobRef)> {
        let record = self.journal.load(owner)?;
        self.instance(&record)?;
        input.run.validate()?;
        if input.run.instance != record.intent.instance || input.run.uri != record.intent.uri
            || record.binding.as_ref().is_some_and(|binding| binding.id != input.run) {
            return Err(BackendError::invalid("collector run differs from retained dispatch"));
        }
        if !matches!(record.request.suite, Suite::DaytimeChecks { .. }) {
            return Err(BackendError::invalid("TL3_CHECKS collector is only for the daytime protocol; full-run rollup is separate"));
        }
        let raw = checked_blob(&input.stdout, &record.intent.output_dir)?;
        let mut frames = raw.split(|byte| *byte == b'\n').filter_map(|line| line.strip_prefix(b"TL3_CHECKS "));
        let payload = frames.next().ok_or_else(|| BackendError::invalid("stdout lacks TL3_CHECKS frame"))?;
        if frames.next().is_some() || payload.is_empty() {
            return Err(BackendError::invalid("stdout has duplicate or empty TL3_CHECKS frames"));
        }
        let _: crate::contracts::report::CheckReport = serde_json::from_slice(payload)?;
        let value: serde_json::Value = serde_json::from_slice(payload)?;
        let mut sanitized = value.clone();
        sanitized_json(&mut sanitized, 0)?;
        if sanitized != value { return Err(BackendError::invalid("TL3_CHECKS frame requires producer redaction or exceeds text bounds")); }
        let out = &record.intent.output_dir;
        let payload = blob(&out.join("retained-payload.json"), payload)?;
        let log = safe(&String::from_utf8_lossy(&raw[..raw.len().min(65_536)]))?;
        let retained = blob(&out.join("retained-stdout.txt"), log.as_bytes())?;
        blob(&out.join("stdout-digests.json"), &serde_json::to_vec(&serde_json::json!({"raw":input.stdout,"retained":retained,"truncated":raw.len()>65_536}))?)?;
        let cleanup_complete = input.cleanup.as_ref().is_some_and(|reference| cleanup_stopped(&record, &input.run, reference).unwrap_or(false));
        let report = RunnerReport {
            schema: "tl3.runner_report.v1".into(), run: input.run.clone(), request_digest: record.intent.request_digest.clone(), tree: record.request.tree.clone(),
            runner_digest: record.request.runner.executable_digest.clone(), runtime_digest: record.request.runtime.manifest_digest.clone(), suite: record.request.suite.clone(),
            complete: true, cleanup_complete, tests_started: 0, outcome: ReportOutcome::ChecksCompleted,
            cases: vec![], payload: Some(payload.clone()), runtime_binding: input.runtime_binding.clone(), cleanup: input.cleanup.clone(), stdout: Some(input.stdout.clone()),
        };
        let report = blob(&out.join("report.json"), &serde_json::to_vec(&report)?)?;
        Ok((report, payload))
    }

    pub(super) fn collect(&self, owner: &Owner, binding: RunBinding) -> Result<Observation> {
        let record = self.journal.load(owner)?;
        if let Some(terminal) = record.terminal { return Ok(Observation::Terminal { binding: Some(binding), terminal }); }
        let collected = (|| -> Result<(Vec<BlobRef>, Option<RunnerReport>)> {
            let out = &record.intent.output_dir;
            let mut evidence = vec![];
            evidence.push(blob(&out.join("events.json"), &serde_json::to_vec(&record.events)?)?);
            let history = self.capture(&["job", "eventlog", "--format=json", &binding.id.job])?;
            if !history.success { return Err(BackendError::unknown("retained native eventlog unavailable")); }
            let mut retained = Vec::new();
            let mut count = 0;
            for (index, line) in history.stdout.split_inclusive(|b| *b == b'\n').enumerate() {
                if line.is_empty() { continue; }
                if record.events.get(index).is_none_or(|event| event.digest != digest(line)) {
                    return Err(BackendError::unknown("exported eventlog differs from retained replay"));
                }
                let mut event: serde_json::Value = serde_json::from_slice(line)?;
                sanitized_json(&mut event, 0)?;
                serde_json::to_writer(&mut retained, &event)?;
                retained.push(b'\n');
                count += 1;
            }
            if count != record.events.len() { return Err(BackendError::unknown("exported eventlog incomplete")); }
            evidence.push(blob(&out.join("eventlog.jsonl"), &retained)?);
            let status = self.capture(&["job", "status", "--json", "--exception-exit-code=125", &binding.id.job])?;
            // The status process may return the task/signal code. Valid JSON is required independently.
            evidence.push(retain_json(out, "native-status.json", &status.stdout)?);
            evidence.push(blob(&out.join("native-status-process.json"), &serde_json::to_vec(&status.status)?)?);
            let jobs = self.capture(&["jobs", "--json", &binding.id.job])?;
            if !jobs.success { return Err(BackendError::unknown("native job result unavailable")); }
            evidence.push(retain_json(out, "native-jobs.json", &jobs.stdout)?);
            let allocation = self.capture(&["job", "info", &binding.id.job, "R"])?;
            if allocation.success { evidence.push(retain_json(out, "native-allocation.json", &allocation.stdout)?); }
            for name in ["stdout", "stderr", "submit.stderr", "submit-status.json"] {
                let path = out.join(name);
                if path.exists() {
                    let bytes = read(&path)?;
                    evidence.push(blob(&out.join(format!("retained-{name}")), safe(&String::from_utf8_lossy(&bytes))?.as_bytes())?);
                }
            }
            let path = out.join("report.json");
            let report = if path.exists() {
                let raw = read(&path)?;
                let report: RunnerReport = serde_json::from_slice(&raw)?;
                if !self.report_identity(&record, &binding, &report)? { return Err(BackendError::unknown("trusted report identity/coverage invalid")); }
                evidence.push(retain_json(out, "retained-report.json", &raw)?);
                if let Some(payload) = &report.payload {
                    let bytes = checked_blob(payload, out)?;
                    evidence.push(retain_json(out, "retained-payload.json", &bytes)?);
                }
                Some(report)
            } else { None };
            Ok((evidence, report))
        })();
        let Ok((evidence, report)) = collected else {
            return Ok(Observation::Unknown { binding: Some(binding), reason: "terminal observation has incomplete, conflicting or unavailable evidence".into() });
        };
        let Some((ending, reason)) = classify(&record, report.as_ref()) else {
            return Ok(Observation::Unknown { binding: Some(binding), reason: "native completion lacks semantic report or authoritative cleanup/loss evidence".into() });
        };
        let report_ref = if report.is_some() {
            Some(blob(&record.intent.output_dir.join("retained-report.json"), &serde_json::to_vec(&report)?)?)
        } else { None };
        let terminal = self.terminal(owner, ending, reason, evidence, report_ref)?;
        Ok(Observation::Terminal { binding: Some(binding), terminal })
    }

    pub(super) fn verified_daytime_payload(&self, owner: &Owner, terminal: &Terminal) -> Result<crate::contracts::report::CheckReport> {
        let record = self.journal.load(owner)?;
        let invalid = || BackendError { code: ErrorCode::EvidenceInvalid, message: "terminal has no verified daytime payload for this durable owner".into() };
        if record.terminal.as_ref() != Some(terminal) || terminal.ending != Ending::Passed
            || !matches!(record.request.suite, Suite::DaytimeChecks { .. }) { return Err(invalid()); }
        let receipt: Receipt = serde_json::from_slice(&checked_blob(&terminal.receipt, &record.intent.output_dir)?)?;
        if receipt.schema != "tl3.run_receipt.v1" || receipt.issuer != ReceiptIssuer::Tl3
            || receipt.intent != record.intent || receipt.binding != record.binding
            || receipt.tree != record.request.tree || receipt.runner != record.request.runner
            || receipt.runtime != record.request.runtime || receipt.reservation != record.request.reservation
            || receipt.ending != terminal.ending || receipt.reason != terminal.reason || receipt.native_events != record.events {
            return Err(invalid());
        }
        let retained_request: Request = serde_json::from_slice(&checked_blob(&receipt.request, &record.intent.output_dir)?)?;
        if retained_request != record.request { return Err(invalid()); }
        let binding = record.binding.as_ref().ok_or_else(invalid)?;
        let reference = receipt.report.as_ref().ok_or_else(invalid)?;
        let report: RunnerReport = serde_json::from_slice(&checked_blob(reference, &record.intent.output_dir)?)?;
        if !self.report_identity(&record, binding, &report)? || report.outcome != ReportOutcome::ChecksCompleted || !report.complete || !report.cleanup_complete {
            return Err(invalid());
        }
        let payload = report.payload.as_ref().ok_or_else(invalid)?;
        Ok(serde_json::from_slice(&checked_blob(payload, &record.intent.output_dir)?)?)
    }

    fn report_identity(&self, record: &Record, binding: &RunBinding, report: &RunnerReport) -> Result<bool> {
        if report.schema != "tl3.runner_report.v1" || report.run != binding.id || report.request_digest != record.intent.request_digest
            || report.tree != record.request.tree || report.runner_digest != record.request.runner.executable_digest
            || report.runtime_digest != record.request.runtime.manifest_digest || report.suite != record.request.suite
            || report.cases.len() > 100_000 {
            return Ok(false);
        }
        if report.cleanup_complete {
            let Some(cleanup) = &report.cleanup else { return Ok(false); };
            if !cleanup_stopped(record, &binding.id, cleanup)? { return Ok(false); }
        }
        if matches!(report.outcome, ReportOutcome::RefusedBeforeTests) {
            return Ok(report.tests_started == 0 && report.cases.is_empty());
        }
        let facts = report.runtime_binding.as_ref();
        if matches!(record.request.runtime.runtime, ExecutionRuntime::RootlessPodman { .. })
            && !facts.is_some_and(|facts| runtime_binding(record, facts).is_ok_and(|result| result.qualified)) {
            return Ok(false);
        }
        if matches!(report.outcome, ReportOutcome::EnvironmentLost) { return Ok(true); }
        if !report.complete { return Ok(false); }
        match &record.request.suite {
            Suite::DaytimeChecks { .. } => {
                let Some(payload) = &report.payload else { return Ok(false); };
                let bytes = checked_blob(payload, &record.intent.output_dir)?;
                let _: crate::contracts::report::CheckReport = serde_json::from_slice(&bytes)?;
                Ok(report.outcome == ReportOutcome::ChecksCompleted)
            }
            Suite::Full { membership_digest } | Suite::SelectedCases { membership_digest } => {
                let bytes = read(&record.intent.output_dir.join("membership.json"))?;
                if digest(&bytes) != *membership_digest { return Ok(false); }
                let membership: Membership = serde_json::from_slice(&bytes)?;
                let expected: BTreeSet<_> = membership.cases.iter().cloned().collect();
                let actual: BTreeSet<_> = report.cases.iter().map(|case| CaseIdentity { target: case.target.clone(), name: case.name.clone() }).collect();
                if membership.schema != "tl3.membership.v1" || expected.len() != membership.cases.len() || actual.len() != report.cases.len() || expected != actual {
                    return Ok(false);
                }
                let mut failed = false;
                for case in &report.cases {
                    if case.target.is_empty() || case.name.is_empty() || case.target.len() > 4096 || case.name.len() > 4096
                        || safe(&case.target)? != case.target || safe(&case.name)? != case.name {
                        return Ok(false);
                    }
                    match case.outcome {
                        CaseOutcome::Failed | CaseOutcome::Error | CaseOutcome::Xpassed => failed = true,
                        CaseOutcome::Skipped | CaseOutcome::Xfailed => {
                            if case.disposition.as_ref().is_none_or(|reason| reason.is_empty() || reason.len() > 4096 || safe(reason).map_or(true, |text| text != *reason)) {
                                return Ok(false);
                            }
                        }
                        CaseOutcome::Passed => {}
                    }
                }
                Ok(if failed { report.outcome == ReportOutcome::TestsFailed } else { report.outcome == ReportOutcome::Passed })
            }
        }
    }
}

fn cleanup_stopped(record: &Record, run: &BackendId, reference: &BlobRef) -> Result<bool> {
    let facts: CleanupFacts = serde_json::from_slice(&checked_blob(reference, &record.intent.output_dir)?)?;
    if facts.schema != "tl3.cleanup_facts.v1" || facts.run != *run || facts.request_digest != record.intent.request_digest
        || facts.scopes.is_empty() || facts.scopes.len() > 1024 { return Ok(false); }
    let mut identities = BTreeSet::new();
    let mut process = false;
    let mut container = false;
    for scope in facts.scopes {
        if scope.identity.is_empty() || scope.identity.len() > 4096 || safe(&scope.identity)? != scope.identity
            || !identities.insert((format!("{:?}", scope.kind), scope.identity)) || scope.present || scope.running == Some(true) { return Ok(false); }
        match scope.kind {
            OwnedScopeKind::ProcessGroup => { if scope.reaped_status.is_none() { return Ok(false); } process = true; }
            OwnedScopeKind::PodmanContainer => container = true,
        }
    }
    Ok(process && (!matches!(record.request.runtime.runtime, ExecutionRuntime::RootlessPodman { .. }) || container))
}

fn classify(record: &Record, report: Option<&RunnerReport>) -> Option<(Ending, &'static str)> {
    if !record.events.iter().any(|event| matches!(event.fact, NativeFact::Clean)) { return None; }
    let started = record.events.iter().position(|event| matches!(event.fact, NativeFact::Start));
    let finish = record.events.iter().enumerate().find_map(|(index, event)| match &event.fact {
        NativeFact::Finish { status } => Some((index, status)), _ => None,
    });
    let cancel = record.events.iter().position(|event| matches!(&event.fact, NativeFact::Exception { exception_type, severity: 0, .. } if exception_type == "cancel"));
    let fatal = record.events.iter().position(|event| matches!(&event.fact, NativeFact::Exception { exception_type, severity: 0, .. } if exception_type != "cancel"));
    if fatal.is_some() {
        if !report.is_some_and(|report| report.cleanup_complete) { return None; }
        return Some((Ending::InfraLost, "authoritative native fatal exception; preserve raw wait status and partial output"));
    }
    let report = report?;
    if !report.cleanup_complete { return None; }
    // A complete finish/report before a later cancellation preserves the original outcome.
    if cancel.is_some() && !finish.is_some_and(|(finish, _)| cancel.is_some_and(|cancel| finish < cancel)) {
        return record.cancel_requested.then_some((Ending::Cancelled, "cancel intent, native cancel exception, clean and trusted stopped-scope evidence"));
    }
    if report.outcome == ReportOutcome::RefusedBeforeTests && report.tests_started == 0 {
        return Some((Ending::Refused, "trusted wrapper refused before tests with terminal cleanup"));
    }
    if report.outcome == ReportOutcome::EnvironmentLost {
        return Some((Ending::InfraLost, "identity-checked trusted runner reports environment failure"));
    }
    let (finished, status) = finish?;
    if !started.is_some_and(|start| start < finished) { return None; }
    match (status, report.outcome) {
        (WaitStatus::Exited { code: 0, .. }, ReportOutcome::Passed | ReportOutcome::ChecksCompleted) =>
            Some((Ending::Passed, "complete identity-checked trusted report; daytime judgment remains in its payload")),
        (WaitStatus::Exited { code: 1, .. }, ReportOutcome::TestsFailed) =>
            Some((Ending::Failed, "complete trusted case coverage identifies semantic test failures")),
        _ => None,
    }
}
