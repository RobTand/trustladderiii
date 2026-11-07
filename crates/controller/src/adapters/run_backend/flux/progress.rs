use super::*;

pub(super) fn historical_phase(fact: &NativeFact) -> NativePhase {
    match fact {
        NativeFact::Start => NativePhase::Running,
        NativeFact::Finish { .. } | NativeFact::Clean => NativePhase::Finishing,
        NativeFact::Other { name }
            if matches!(
                name.as_str(),
                "submit" | "validate" | "depend" | "priority" | "alloc"
            ) =>
        {
            NativePhase::Queued
        }
        _ => NativePhase::Unknown,
    }
}

impl Flux {
    pub(super) fn observe_liveness(&self, owner: &Owner) -> Result<Liveness> {
        let record = self.journal.load(owner)?;
        let observed_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .unwrap_or(0);
        let cursor = record.events.last().map(|event| EventCursor {
            sequence: event.sequence,
            digest: event.digest.clone(),
        });
        let mut observation = Liveness { owner: owner.clone(), run: record.binding.as_ref().map(|binding| binding.id.clone()), cursor,
            phase: NativePhase::Unknown, authority: ProgressAuthority::Unknown, observed_unix_ms, terminal: None, evidence: None,
            reason: "native liveness unavailable; retained events and local watcher do not prove current execution".into() };
        if let Some(terminal) = record.terminal {
            observation.phase = NativePhase::Terminal;
            observation.authority = ProgressAuthority::CommittedTerminal;
            observation.terminal = Some(terminal);
            return Ok(observation);
        }
        if self.instance(&record).is_err() {
            return Ok(observation);
        }
        // Scheduled Look observes an exact durable ID. Orphan reconciliation belongs to attach.
        let Some(binding) = record.binding.clone() else {
            return Ok(observation);
        };
        observation.run = Some(binding.id.clone());
        let queried = (|| -> Result<(NativePhase, BlobRef)> {
            let reply = self.capture(&["jobs", "--json", &binding.id.job])?;
            if !reply.success {
                return Err(BackendError::unknown("native liveness query failed"));
            }
            // Flux 0.91 emits a bare object for exactly one requested ID; list queries use {jobs:[]}.
            let job: process::Job = serde_json::from_slice(&reply.stdout)?;
            if job.jobid != binding.id.job || job.name != record.intent.correlation {
                return Err(BackendError::unknown(
                    "native liveness identity missing or ambiguous",
                ));
            }
            let phase = match job.state.as_deref() {
                Some("DEPEND" | "PRIORITY" | "SCHED") => NativePhase::Queued,
                Some("RUN") => NativePhase::Running,
                Some("CLEANUP" | "INACTIVE") => NativePhase::Finishing,
                _ => return Err(BackendError::unknown("unknown native state")),
            };
            self.ensure_generation()?;
            let evidence = evidence::retain_json(
                &record.intent.output_dir,
                &format!("liveness-{}.json", digest(&reply.stdout)),
                &reply.stdout,
            )?;
            Ok((phase, evidence))
        })();
        // Re-read after native I/O. A committed terminal wins over a racing active snapshot.
        let current = self.journal.load(owner)?;
        observation.cursor = current.events.last().map(|event| EventCursor {
            sequence: event.sequence,
            digest: event.digest.clone(),
        });
        if let Some(terminal) = current.terminal {
            observation.phase = NativePhase::Terminal;
            observation.authority = ProgressAuthority::CommittedTerminal;
            observation.terminal = Some(terminal);
        } else if current.binding.as_ref() == Some(&binding)
            && let Ok((phase, evidence)) = queried
        {
            observation.phase = phase;
            observation.authority = ProgressAuthority::FreshNative;
            observation.evidence = Some(evidence);
            observation.reason = "one fresh native state read on the pinned generation; historical progress is separate".into();
        }
        Ok(observation)
    }
}
