use super::*;
use std::collections::BTreeSet;

fn unqualified(reason: &str) -> RuntimeQualification {
    RuntimeQualification {
        qualified: false,
        reason: reason.into(),
        evidence: vec![],
    }
}

pub(super) fn allocation_grant(record: &Record, facts: &AllocationInput) -> Result<CpuGrant> {
    facts.run.validate()?;
    cgroup_path(&facts.host.cgroup)?;
    if facts.run.instance != record.intent.instance
        || facts.run.uri != record.intent.uri
        || facts.request_digest != record.intent.request_digest
        || record
            .binding
            .as_ref()
            .is_some_and(|binding| binding.id != facts.run)
        || !record.dispatched
    {
        return Err(BackendError::invalid(
            "allocation differs from retained dispatch",
        ));
    }
    let r: resources::ResourceSet =
        serde_json::from_slice(&checked_blob(&facts.allocation, &record.intent.output_dir)?)?;
    let topology: TopologyMapping =
        serde_json::from_slice(&checked_blob(&facts.topology, &record.intent.output_dir)?)?;
    if r.version != 1
        || r.execution.ranks.len() != 1
        || topology.run != facts.run
        || topology.request_digest != record.intent.request_digest
        || topology.ranks.len() > 4096
        || facts.host.host.len() > 256
        || !token(&facts.host.host)
    {
        return Err(BackendError::invalid(
            "runtime allocation/topology identity mismatch",
        ));
    }
    // This first runtime is one native task on one rank, never an inferred cross-rank mask.
    let allocated = r
        .execution
        .ranks
        .iter()
        .filter_map(|rank| {
            resources::idset(&rank.rank)
                .ok()
                .filter(|ranks| ranks.contains(&facts.host.rank))
                .map(|_| rank)
        })
        .collect::<Vec<_>>();
    if allocated.len() != 1 {
        return Err(BackendError::invalid(
            "native rank allocation missing or ambiguous",
        ));
    }
    let mappings = topology
        .ranks
        .iter()
        .filter(|rank| rank.rank == facts.host.rank && rank.host == facts.host.host)
        .collect::<Vec<_>>();
    if mappings.len() != 1 {
        return Err(BackendError::invalid(
            "native core to OS-PU mapping missing or ambiguous",
        ));
    }
    let cores = resources::idset(&allocated[0].children.core)?;
    if resources::idset(&allocated[0].rank)? != BTreeSet::from([facts.host.rank])
        || cores.len() as u64 != u64::from(record.request.reservation.cpus.get())
        || resources::idset(&allocated[0].children.gpu)?.len() as u64
            != u64::from(record.request.reservation.gpus)
    {
        return Err(BackendError::invalid(
            "observed core grant differs from declared task shape",
        ));
    }
    let mut pus = BTreeSet::new();
    let mut mapped = BTreeSet::new();
    for core in &mappings[0].cores {
        if cores.contains(&core.flux_core) {
            if !mapped.insert(core.flux_core) {
                return Err(BackendError::invalid("duplicate native core mapping"));
            }
            let core_pus = resources::idset(&core.os_pus)?;
            if core_pus.is_empty() || core_pus.iter().any(|pu| !pus.insert(*pu)) {
                return Err(BackendError::invalid("invalid overlapping OS-PU mapping"));
            }
        }
    }
    if mapped != cores || resources::idset(&facts.host.allowed_cpus)? != pus {
        return Err(BackendError::invalid(
            "host CPU affinity does not match mapped native grant",
        ));
    }
    if facts.host.host != "dl380g10" || facts.host.rank != 0 {
        return Err(BackendError::invalid(
            "runtime host differs from first-cut placement",
        ));
    }
    Ok(CpuGrant {
        run: facts.run.clone(),
        request_digest: facts.request_digest.clone(),
        host: facts.host.host.clone(),
        rank: facts.host.rank,
        os_pus: pus.into_iter().collect(),
        allocation: facts.allocation.clone(),
        topology: facts.topology.clone(),
    })
}

pub(super) fn runtime_binding(
    record: &Record,
    facts: &RuntimeBindingInput,
) -> Result<RuntimeQualification> {
    let Some(binding) = &record.binding else {
        return Ok(unqualified("backend ID not yet durably bound"));
    };
    if facts.run != binding.id || facts.request_digest != record.intent.request_digest {
        return Ok(unqualified(
            "runtime facts do not match retained backend identity",
        ));
    }
    let checked = (|| -> Result<()> {
        let grant = allocation_grant(
            record,
            &AllocationInput {
                run: facts.run.clone(),
                request_digest: facts.request_digest.clone(),
                allocation: facts.allocation.clone(),
                topology: facts.topology.clone(),
                host: facts.host.clone(),
            },
        )?;
        let pus: BTreeSet<_> = grant.os_pus.into_iter().collect();
        let cgroup = cgroup_path(&facts.host.cgroup)?;
        if let ExecutionRuntime::RootlessPodman {
            mode,
            memory_limit_bytes,
            ..
        } = record.request.runtime.runtime
        {
            let owned = record
                .runtime_ownerships
                .iter()
                .find(|owned| owned.execution == facts.execution)
                .and_then(|owned| owned.container.as_ref())
                .ok_or_else(|| BackendError::invalid("container execution ownership missing"))?;
            if facts.container_id.as_ref() != Some(&owned.id) {
                return Err(BackendError::invalid(
                    "observed container identity differs from retained execution",
                ));
            }
            let container = facts.container.as_ref().ok_or_else(|| {
                BackendError::invalid("inside-container affinity/cgroup evidence missing")
            })?;
            if container.rank != facts.host.rank
                || container.host != facts.host.host
                || resources::idset(&container.allowed_cpus)? != pus
            {
                return Err(BackendError::invalid(
                    "container CPU affinity differs from native grant",
                ));
            }
            let child = cgroup_path(&container.cgroup)?;
            match mode {
                ContainerMode::InJob => {
                    if child != cgroup
                        && !child
                            .strip_prefix(&cgroup)
                            .is_some_and(|suffix| suffix.starts_with('/'))
                    {
                        return Err(BackendError::invalid(
                            "in_job container escaped host cgroup",
                        ));
                    }
                }
                ContainerMode::OwnScope => {
                    let scope = facts.container_scope.as_ref().ok_or_else(|| {
                        BackendError::invalid("own_scope unit/cpuset/memory evidence missing")
                    })?;
                    let owned = record
                        .runtime_ownerships
                        .iter()
                        .find(|ownership| ownership.execution == facts.execution)
                        .and_then(|ownership| ownership.container.as_ref())
                        .ok_or_else(|| BackendError::invalid("container ownership not retained"))?;
                    let retained_scope = owned
                        .scope
                        .as_ref()
                        .ok_or_else(|| BackendError::invalid("scope ownership not retained"))?;
                    if scope.container_id != owned.id
                        || retained_scope.unit != scope.unit
                        || retained_scope.invocation_id != scope.invocation_id
                        || retained_scope.cgroup != child
                        || !scope.unit.ends_with(".scope")
                        || !token(&scope.unit)
                        || !hex(&scope.invocation_id, 32)
                        || cgroup_path(&scope.cgroup)? != child
                        || child == cgroup
                        || resources::idset(&scope.cpuset_effective)? != pus
                        || scope.memory_max != memory_limit_bytes.map(|bytes| bytes.get())
                        || scope.memory_swap_max != Some(0)
                    {
                        return Err(BackendError::invalid(
                            "own_scope generation/cpuset/per-container memory cap mismatch",
                        ));
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
    let line = lines
        .next()
        .ok_or_else(|| BackendError::invalid("cgroup facts missing"))?;
    let path = line
        .strip_prefix("0::")
        .ok_or_else(|| BackendError::invalid("unsupported non-unified cgroup facts"))?;
    if lines.next().is_some() || path == "/" || !absolute(Path::new(path)) || path.len() > 4096 {
        return Err(BackendError::invalid("unqualified job cgroup path"));
    }
    Ok(path.into())
}

fn sanitized_json(value: &mut serde_json::Value, depth: usize) -> Result<()> {
    sanitized_json_using(value, depth, safe)
}

pub(super) fn sanitized_json_using(
    value: &mut serde_json::Value,
    depth: usize,
    text: fn(&str) -> Result<String>,
) -> Result<()> {
    fn visit(
        value: &mut serde_json::Value,
        depth: usize,
        text: fn(&str) -> Result<String>,
        strings: &mut std::collections::HashMap<String, String>,
        names: &mut BTreeSet<String>,
    ) -> Result<()> {
        if depth > 32 {
            return Err(BackendError::invalid("evidence nesting exceeds bound"));
        }
        match value {
            serde_json::Value::String(value) => {
                if let Some(checked) = strings.get(value) {
                    *value = checked.clone();
                } else {
                    let checked = text(value)?;
                    strings.insert(value.clone(), checked.clone());
                    *value = checked;
                }
            }
            serde_json::Value::Array(values) => {
                if values.len() > 100_000 {
                    return Err(BackendError::invalid("evidence collection exceeds bound"));
                }
                for value in values {
                    visit(value, depth + 1, text, strings, names)?;
                }
            }
            serde_json::Value::Object(values) => {
                if values.len() > 256 {
                    return Err(BackendError::invalid("evidence fields exceed bound"));
                }
                for (name, value) in values {
                    if !names.contains(name) {
                        if name.len() > 256 || safe(name)? != *name {
                            return Err(BackendError::invalid("unsafe evidence field name"));
                        }
                        names.insert(name.clone());
                    }
                    visit(value, depth + 1, text, strings, names)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(
        value,
        depth,
        text,
        &mut std::collections::HashMap::new(),
        &mut BTreeSet::new(),
    )
}

fn daytime_payload(bytes: &[u8]) -> Result<crate::contracts::report::CheckReport> {
    if bytes.len() > MAX_BYTES {
        return Err(BackendError::invalid(
            "daytime payload exceeds protocol bound",
        ));
    }
    let report = serde_json::from_slice(bytes)?;
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let mut sanitized = value.clone();
    sanitized_json_using(&mut sanitized, 0, safe_report)?;
    if sanitized != value {
        return Err(BackendError::invalid(
            "TL3_CHECKS frame requires producer redaction or exceeds report bounds",
        ));
    }
    Ok(report)
}

pub(super) fn retain_json(out: &Path, name: &str, bytes: &[u8]) -> Result<BlobRef> {
    let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
    sanitized_json(&mut value, 0)?;
    blob(&out.join(name), &serde_json::to_vec(&value)?)
}

impl Flux {
    /// Convert only the trusted host stdout protocol. Existing daytime judging remains in FX2.
    pub fn collect_runner(
        &self,
        owner: &Owner,
        input: &CollectorInput,
    ) -> Result<(BlobRef, BlobRef)> {
        let record = self.journal.load(owner)?;
        self.instance(&record)?;
        validate::trusted_artifacts(&record.request)?;
        input.run.validate()?;
        if record.terminal.is_some() {
            return Err(BackendError::invalid(
                "collector cannot replace committed terminal evidence",
            ));
        }
        if input.run.instance != record.intent.instance
            || input.run.uri != record.intent.uri
            || !record.dispatched
            || record
                .binding
                .as_ref()
                .is_none_or(|binding| binding.id != input.run)
        {
            return Err(BackendError::invalid(
                "collector run differs from retained dispatch",
            ));
        }
        if !matches!(record.request.suite, Suite::DaytimeChecks { .. }) {
            return Err(BackendError::invalid(
                "TL3_CHECKS collector is only for the daytime protocol; full-run rollup is separate",
            ));
        }
        let bounds = output_bounds(&record.request.suite);
        let raw = checked_blob_limit(
            &input.stdout,
            &record.intent.output_dir,
            bounds.stdout_bytes,
        )?;
        if let Some(stderr) = &input.stderr {
            checked_blob_limit(stderr, &record.intent.output_dir, bounds.stderr_bytes)?;
        }
        let mut frames = raw
            .split(|byte| *byte == b'\n')
            .filter_map(|line| line.strip_prefix(b"TL3_CHECKS "));
        let payload = frames
            .next()
            .ok_or_else(|| BackendError::invalid("stdout lacks TL3_CHECKS frame"))?;
        if frames.next().is_some() || payload.is_empty() {
            return Err(BackendError::invalid(
                "stdout has duplicate or empty TL3_CHECKS frames",
            ));
        }
        if payload.len() > bounds.payload_bytes {
            return Err(BackendError::invalid(
                "TL3_CHECKS payload exceeds protocol byte bound",
            ));
        }
        daytime_payload(payload)?;
        let out = &record.intent.output_dir;
        let payload = blob(&out.join("retained-payload.json"), payload)?;
        let log = safe(&String::from_utf8_lossy(&raw[..raw.len().min(65_536)]))?;
        let retained = blob(&out.join("retained-stdout.txt"), log.as_bytes())?;
        blob(
            &out.join("stdout-digests.json"),
            &serde_json::to_vec(
                &serde_json::json!({"raw":input.stdout,"retained":retained,"truncated":raw.len()>65_536}),
            )?,
        )?;
        if let Some(reference) = &input.cleanup {
            self.retain_cleanup(owner, reference)?;
        }
        let cleanup_complete = super::cleanup::all_stopped(&self.journal.load(owner)?);
        let report = RunnerReport {
            schema: "tl3.runner_report.v1".into(),
            run: input.run.clone(),
            request_digest: record.intent.request_digest.clone(),
            tree: record.request.tree.clone(),
            runner_digest: record.request.runner.executable_digest.clone(),
            runtime_digest: record.request.runtime.manifest_digest.clone(),
            suite: record.request.suite.clone(),
            complete: true,
            cleanup_complete,
            tests_started: 0,
            outcome: ReportOutcome::ChecksCompleted,
            cases: vec![],
            payload: Some(payload.clone()),
            runtime_binding: input.runtime_binding.clone(),
            runtime_bindings: input.runtime_bindings.clone(),
            cleanup: input.cleanup.clone(),
            stdout: Some(input.stdout.clone()),
            stderr: input.stderr.clone(),
            worker_payload: None,
        };
        let report = blob(&out.join("report.json"), &serde_json::to_vec(&report)?)?;
        Ok((report, payload))
    }

    pub(super) fn collect(&self, owner: &Owner, binding: RunBinding) -> Result<Observation> {
        let record = self.journal.load(owner)?;
        if let Some(terminal) = record.terminal {
            return Ok(Observation::Terminal {
                binding: Some(binding),
                terminal,
            });
        }
        let collected = (|| -> Result<(Vec<BlobRef>, Option<RunnerReport>)> {
            let out = &record.intent.output_dir;
            let mut evidence = vec![];
            if let Some(spec) = &record.jobspec {
                checked_blob(spec, out)?;
                evidence.push(spec.clone());
            }
            for cleanup in record.cleanup_evidence.values() {
                checked_blob(cleanup, out)?;
                evidence.push(cleanup.clone());
            }
            evidence.push(blob(
                &out.join("events.json"),
                &serde_json::to_vec(&record.events)?,
            )?);
            let history = self.capture(&["job", "eventlog", "--format=json", &binding.id.job])?;
            if !history.success {
                return Err(BackendError::unknown(
                    "retained native eventlog unavailable",
                ));
            }
            let mut retained = Vec::new();
            let mut count = 0;
            for (index, line) in history.stdout.split_inclusive(|b| *b == b'\n').enumerate() {
                if line.is_empty() {
                    continue;
                }
                if record
                    .events
                    .get(index)
                    .is_none_or(|event| event.digest != digest(line))
                {
                    return Err(BackendError::unknown(
                        "exported eventlog differs from retained replay",
                    ));
                }
                let mut event: serde_json::Value = serde_json::from_slice(line)?;
                sanitized_json(&mut event, 0)?;
                serde_json::to_writer(&mut retained, &event)?;
                retained.push(b'\n');
                count += 1;
            }
            if count != record.events.len() {
                return Err(BackendError::unknown("exported eventlog incomplete"));
            }
            evidence.push(blob(&out.join("eventlog.jsonl"), &retained)?);
            let status = self.capture(&[
                "job",
                "status",
                "--json",
                "--exception-exit-code=125",
                &binding.id.job,
            ])?;
            // The status process may return the task/signal code. Valid JSON is required independently.
            evidence.push(retain_json(out, "native-status.json", &status.stdout)?);
            evidence.push(blob(
                &out.join("native-status-process.json"),
                &serde_json::to_vec(&status.status)?,
            )?);
            let jobs = self.capture(&["jobs", "--json", &binding.id.job])?;
            if !jobs.success {
                return Err(BackendError::unknown("native job result unavailable"));
            }
            evidence.push(retain_json(out, "native-jobs.json", &jobs.stdout)?);
            let allocation = self.capture(&["job", "info", &binding.id.job, "R"])?;
            if allocation.success {
                evidence.push(retain_json(
                    out,
                    "native-allocation.json",
                    &allocation.stdout,
                )?);
            }
            for name in ["stdout", "stderr", "submit.stderr", "submit-status.json"] {
                let path = out.join(name);
                if path.exists() {
                    let bounds = output_bounds(&record.request.suite);
                    let limit = match name {
                        "stdout" => bounds.stdout_bytes,
                        "stderr" => bounds.stderr_bytes,
                        _ => MAX_BYTES,
                    };
                    let bytes = read_limit(&path, limit)?;
                    evidence.push(blob(
                        &out.join(format!("retained-{name}")),
                        safe(&String::from_utf8_lossy(&bytes))?.as_bytes(),
                    )?);
                }
            }
            let path = out.join("report.json");
            let report = if path.exists() {
                let raw = read(&path)?;
                let report: RunnerReport = serde_json::from_slice(&raw)?;
                if !self.report_identity(&record, &binding, &report)? {
                    return Err(BackendError::unknown(
                        "trusted report identity/coverage invalid",
                    ));
                }
                evidence.push(blob(&out.join("retained-report.json"), &raw)?);
                if let Some(payload) = &report.payload {
                    let bytes = checked_blob(payload, out)?;
                    if matches!(report.suite, Suite::DaytimeChecks { .. }) {
                        daytime_payload(&bytes)?;
                        evidence.push(blob(&out.join("retained-payload.json"), &bytes)?);
                    } else {
                        evidence.push(retain_json(out, "retained-payload.json", &bytes)?);
                    }
                }
                Some(report)
            } else {
                None
            };
            Ok((evidence, report))
        })();
        let Ok((evidence, report)) = collected else {
            self.mark_cleanup_debt(&record)?;
            return Ok(Observation::Unknown {
                binding: Some(binding),
                reason: "terminal observation has incomplete, conflicting or unavailable evidence"
                    .into(),
            });
        };
        let record = self.journal.load(owner)?;
        let cleanup_complete = super::cleanup::all_stopped(&record);
        if cleanup_complete
            && record.runtime_ownerships.is_empty()
            && record.container_launches.is_empty()
        {
            // A queued cancel or wrapper failure may have retained unregistered
            // debt before native clean arrived. Retire only that exact intent.
            self.run_cleanup(owner)?;
        }
        let Some((ending, reason)) = classify(&record, report.as_ref(), cleanup_complete) else {
            self.mark_cleanup_debt(&record)?;
            return Ok(Observation::Unknown {
                binding: Some(binding),
                reason:
                    "native completion lacks semantic report or authoritative cleanup/loss evidence"
                        .into(),
            });
        };
        let report_ref = if report.is_some() {
            let path = record.intent.output_dir.join("retained-report.json");
            Some(blob(&path, &read(&path)?)?)
        } else {
            None
        };
        let terminal = self.terminal(owner, ending, reason, evidence, report_ref)?;
        Ok(Observation::Terminal {
            binding: Some(binding),
            terminal,
        })
    }

    pub(super) fn verified_report(
        &self,
        owner: &Owner,
        terminal: &Terminal,
    ) -> Result<RunnerReport> {
        let record = self.journal.load(owner)?;
        let invalid = || BackendError {
            code: ErrorCode::EvidenceInvalid,
            message: "terminal has no verified payload for this durable owner".into(),
        };
        if record.terminal.as_ref() != Some(terminal)
            || !matches!(terminal.ending, Ending::Passed | Ending::Failed)
        {
            return Err(invalid());
        }
        let receipt: Receipt =
            serde_json::from_slice(&checked_blob(&terminal.receipt, &record.intent.output_dir)?)?;
        if receipt.schema != "tl3.run_receipt.v1"
            || receipt.issuer != ReceiptIssuer::Tl3
            || receipt.intent != record.intent
            || receipt.binding != record.binding
            || receipt.tree != record.request.tree
            || receipt.runner != record.request.runner
            || receipt.runtime != record.request.runtime
            || receipt.reservation != record.request.reservation
            || receipt.runtime_ownerships != record.runtime_ownerships
            || receipt.container_launches
                != record
                    .container_launches
                    .values()
                    .cloned()
                    .collect::<Vec<_>>()
            || receipt.cleanup != record.cleanup_evidence
            || receipt.ending != terminal.ending
            || receipt.reason != terminal.reason
            || receipt.native_events != record.events
        {
            return Err(invalid());
        }
        let retained_request: Request =
            serde_json::from_slice(&checked_blob(&receipt.request, &record.intent.output_dir)?)?;
        if retained_request != record.request {
            return Err(invalid());
        }
        let binding = record.binding.as_ref().ok_or_else(invalid)?;
        let reference = receipt.report.as_ref().ok_or_else(invalid)?;
        let report: RunnerReport =
            serde_json::from_slice(&checked_blob(reference, &record.intent.output_dir)?)?;
        if !self.report_identity(&record, binding, &report)?
            || !report.complete
            || !report.cleanup_complete
        {
            return Err(invalid());
        }
        Ok(report)
    }

    pub(super) fn verified_daytime_payload(
        &self,
        owner: &Owner,
        terminal: &Terminal,
    ) -> Result<crate::contracts::report::CheckReport> {
        let report = self.verified_report(owner, terminal)?;
        if !matches!(report.suite, Suite::DaytimeChecks { .. })
            || report.outcome != ReportOutcome::ChecksCompleted
        {
            return Err(BackendError::invalid(
                "terminal is not a daytime check payload",
            ));
        }
        let payload = report
            .payload
            .as_ref()
            .ok_or_else(|| BackendError::invalid("daytime payload missing"))?;
        let record = self.journal.load(owner)?;
        daytime_payload(&checked_blob(payload, &record.intent.output_dir)?)
    }

    pub(super) fn report_identity(
        &self,
        record: &Record,
        binding: &RunBinding,
        report: &RunnerReport,
    ) -> Result<bool> {
        validate::trusted_artifacts(&record.request)?;
        let bounds = output_bounds(&record.request.suite);
        if let Some(stdout) = &report.stdout {
            checked_blob_limit(stdout, &record.intent.output_dir, bounds.stdout_bytes)?;
        }
        if let Some(stderr) = &report.stderr {
            checked_blob_limit(stderr, &record.intent.output_dir, bounds.stderr_bytes)?;
        }
        if report.schema != "tl3.runner_report.v1"
            || report.run != binding.id
            || report.request_digest != record.intent.request_digest
            || report.tree != record.request.tree
            || report.runner_digest != record.request.runner.executable_digest
            || report.runtime_digest != record.request.runtime.manifest_digest
            || report.suite != record.request.suite
            || report.cases.len() > 100_000
        {
            return Ok(false);
        }
        if report.cleanup_complete {
            if let Some(cleanup) = &report.cleanup
                && !cleanup_stopped(record, &binding.id, cleanup)?
            {
                return Ok(false);
            }
            if !super::cleanup::all_stopped(record) {
                return Ok(false);
            }
        }
        if matches!(report.outcome, ReportOutcome::RefusedBeforeTests) {
            return Ok(report.tests_started == 0 && report.cases.is_empty());
        }
        if matches!(
            record.request.runtime.runtime,
            ExecutionRuntime::RootlessPodman { .. }
        ) {
            let facts: Vec<_> = report
                .runtime_binding
                .iter()
                .chain(report.runtime_bindings.iter())
                .collect();
            let containers: Vec<_> = record
                .runtime_ownerships
                .iter()
                .filter(|owned| owned.container.is_some())
                .collect();
            let mut observed = BTreeSet::new();
            if containers.is_empty()
                || facts.iter().any(|facts| {
                    !observed.insert(&facts.execution)
                        || !runtime_binding(record, facts).is_ok_and(|result| result.qualified)
                })
                || containers
                    .iter()
                    .any(|owned| !observed.contains(&owned.execution))
            {
                return Ok(false);
            }
        }
        if matches!(report.outcome, ReportOutcome::EnvironmentLost) {
            return Ok(true);
        }
        if !report.complete {
            return Ok(false);
        }
        match &record.request.suite {
            Suite::WorkerTests { .. } | Suite::Generator { .. } => {
                super::worker_protocol::report_identity(record, report)
            }
            Suite::DaytimeChecks { .. } => {
                let Some(payload) = &report.payload else {
                    return Ok(false);
                };
                let bytes = checked_blob(payload, &record.intent.output_dir)?;
                daytime_payload(&bytes)?;
                Ok(report.outcome == ReportOutcome::ChecksCompleted)
            }
            Suite::Full { membership_digest } | Suite::SelectedCases { membership_digest } => {
                let bytes = read(&record.intent.output_dir.join("membership.json"))?;
                if digest(&bytes) != *membership_digest {
                    return Ok(false);
                }
                let membership: Membership = serde_json::from_slice(&bytes)?;
                let expected: BTreeSet<_> = membership.cases.iter().cloned().collect();
                let actual: BTreeSet<_> = report
                    .cases
                    .iter()
                    .map(|case| CaseIdentity {
                        target: case.target.clone(),
                        name: case.name.clone(),
                    })
                    .collect();
                if membership.schema != "tl3.membership.v1"
                    || expected.len() != membership.cases.len()
                    || actual.len() != report.cases.len()
                    || expected != actual
                {
                    return Ok(false);
                }
                let mut failed = false;
                for case in &report.cases {
                    if case.target.is_empty()
                        || case.name.is_empty()
                        || case.target.len() > 4096
                        || case.name.len() > 4096
                        || safe(&case.target)? != case.target
                        || safe(&case.name)? != case.name
                    {
                        return Ok(false);
                    }
                    match case.outcome {
                        CaseOutcome::Failed | CaseOutcome::Error | CaseOutcome::Xpassed => {
                            failed = true
                        }
                        CaseOutcome::Skipped | CaseOutcome::Xfailed => {
                            if case.disposition.as_ref().is_none_or(|reason| {
                                reason.is_empty()
                                    || reason.len() > 4096
                                    || safe(reason).map_or(true, |text| text != *reason)
                            }) {
                                return Ok(false);
                            }
                        }
                        CaseOutcome::Passed => {}
                    }
                }
                Ok(if failed {
                    report.outcome == ReportOutcome::TestsFailed
                } else {
                    report.outcome == ReportOutcome::Passed
                })
            }
        }
    }
}

fn cleanup_stopped(record: &Record, run: &BackendId, reference: &BlobRef) -> Result<bool> {
    super::cleanup::stopped(record, run, reference)
}

fn classify(
    record: &Record,
    report: Option<&RunnerReport>,
    cleanup_complete: bool,
) -> Option<(Ending, &'static str)> {
    if !record
        .events
        .iter()
        .any(|event| matches!(event.fact, NativeFact::Clean))
    {
        return None;
    }
    let started = record
        .events
        .iter()
        .position(|event| matches!(event.fact, NativeFact::Start));
    let finish = record
        .events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match &event.fact {
            NativeFact::Finish { status } => Some((index, status)),
            _ => None,
        });
    let cancel = record.events.iter().position(|event| matches!(&event.fact, NativeFact::Exception { exception_type, severity: 0, .. } if exception_type == "cancel"));
    let fatal = record.events.iter().position(|event| matches!(&event.fact, NativeFact::Exception { exception_type, severity: 0, .. } if exception_type != "cancel"));
    if !cleanup_complete {
        return None;
    }
    if fatal.is_some() {
        return Some((
            Ending::InfraLost,
            "authoritative native fatal exception; preserve raw wait status and partial output",
        ));
    }
    // A complete finish/report before a later cancellation preserves the original outcome.
    if cancel.is_some()
        && !finish.is_some_and(|(finish, _)| cancel.is_some_and(|cancel| finish < cancel))
    {
        return record.cancel_requested.then_some((
            Ending::Cancelled,
            "cancel intent, native cancel exception, clean and trusted stopped-scope evidence",
        ));
    }
    if finish.is_some_and(|(_, status)| {
        matches!(
            status,
            WaitStatus::Exited {
                code: 137 | 143,
                ..
            } | WaitStatus::Signalled { signal: 9 | 15, .. }
        )
    }) {
        return Some((
            Ending::InfraLost,
            "infrastructure/resource termination; preserve raw status and partial output; OOM cause is not inferred",
        ));
    }
    if report.is_none()
        && record.runtime_ownerships.is_empty()
        && matches!(
            record.request.runtime.runtime,
            ExecutionRuntime::RootlessPodman {
                mode: ContainerMode::InJob,
                ..
            }
        )
        && finish.is_some_and(|(_, status)| !matches!(status, WaitStatus::Exited { code: 0, .. }))
    {
        return Some((
            Ending::InfraLost,
            "native wrapper failed before any container binding; all pending launchers stopped",
        ));
    }
    let report = report?;
    if report.outcome == ReportOutcome::RefusedBeforeTests && report.tests_started == 0 {
        return Some((
            Ending::Refused,
            "trusted wrapper refused before tests with terminal cleanup",
        ));
    }
    if report.outcome == ReportOutcome::EnvironmentLost {
        return Some((
            Ending::InfraLost,
            "identity-checked trusted runner reports environment failure",
        ));
    }
    let (finished, status) = finish?;
    if !started.is_some_and(|start| start < finished) {
        return None;
    }
    match (status, report.outcome) {
        (
            WaitStatus::Exited { code: 0, .. },
            ReportOutcome::Passed
            | ReportOutcome::ChecksCompleted
            | ReportOutcome::WorkerPayloadCompleted,
        ) => Some((
            Ending::Passed,
            "complete identity-checked trusted report; daytime judgment remains in its payload",
        )),
        (WaitStatus::Exited { code: 1, .. }, ReportOutcome::TestsFailed) => Some((
            Ending::Failed,
            "complete trusted case coverage identifies semantic test failures",
        )),
        _ => None,
    }
}
