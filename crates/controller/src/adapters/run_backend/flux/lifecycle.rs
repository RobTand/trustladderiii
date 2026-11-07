use super::*;

pub(super) fn labels(owned: &RuntimeOwnership) -> std::collections::BTreeMap<String, String> {
    [
        ("tl3.backend", "flux"),
        ("flux.jobid", owned.run.job.as_str()),
        ("tl3.execution", owned.execution.as_str()),
        ("tl3.uri", owned.run.uri.as_str()),
        ("tl3.instance", owned.run.instance.as_str()),
        ("tl3.job", owned.run.job.as_str()),
        ("tl3.owner", owned.owner.owner.as_str()),
        ("tl3.correlation", owned.owner.correlation.as_str()),
        ("tl3.request_digest", owned.request_digest.as_str()),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect()
}

pub(super) fn matches(intent: &ContainerLaunchIntent, observed: &RuntimeOwnership) -> bool {
    let mut launcher = observed.clone();
    launcher.container = None;
    launcher == intent.launcher
        && observed.container.as_ref().is_some_and(|container| {
            container.name == intent.name
                && container.mode == intent.mode
                && container.labels == intent.labels
        })
}

impl Flux {
    pub(super) fn begin_launch(
        &self,
        owner: &Owner,
        launcher: &RuntimeOwnership,
    ) -> Result<ContainerLaunchIntent> {
        self.journal.runtime_gate(|debts| {
        self.journal.transaction(owner, |dir, current| {
            let mut current = current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
            self.instance(&current)?;
            cleanup::ownership(&current, launcher)?;
            let ExecutionRuntime::RootlessPodman { mode, .. } = current.request.runtime.runtime else {
                return Err(BackendError::invalid("container launch requires the selected Podman runtime"));
            };
            if launcher.container.is_some() || current.binding.as_ref().is_none_or(|binding| binding.id != launcher.run) {
                return Err(BackendError::invalid("prelaunch requires launcher-only facts and the exact retained job binding"));
            }
            let intent = ContainerLaunchIntent { schema: "tl3.container_launch.v1".into(), launcher: launcher.clone(),
                name: format!("tl3-{}", digest(&serde_json::to_vec(&(owner, &launcher.run, &launcher.request_digest, &launcher.execution))?)),
                mode, labels: labels(launcher) };
            if current.cancel_requested || current.terminal.is_some() {
                return Err(BackendError::unknown("container launch is held after cancellation or terminal"));
            }
            if let Some(existing) = current.container_launches.get(&launcher.execution) {
                return Err(BackendError { code: ErrorCode::Conflict, message: if existing == &intent { "launch intent already retained; never invoke Podman again, reconcile/cleanup" } else { "prelaunch identity is immutable" }.into() });
            }
            if current.container_launches.len() >= 1024 || current.runtime_ownerships.iter().any(|owned| owned.execution == launcher.execution) {
                return Err(BackendError::invalid("launch execution already registered or limit reached"));
            }
            let runtime = debts.join(&current.request.runtime.manifest_digest);
            std::fs::create_dir_all(&runtime)?;
            atomic(&runtime.join(format!("{}.{}", current.intent.correlation, launcher.execution)), &serde_json::to_vec(&current.intent)?)?;
            current.container_launches.insert(launcher.execution.clone(), intent.clone());
            save(dir, &current)?;
            Ok(intent)
        })
        })
    }

    pub(super) fn bind_launch(
        &self,
        owner: &Owner,
        observed: RuntimeOwnership,
    ) -> Result<ContainerLaunchBinding> {
        self.journal.transaction(owner, |dir, current| {
            let mut current = current.ok_or_else(|| BackendError::unknown("intent disappeared"))?;
            self.instance(&current)?;
            cleanup::ownership(&current, &observed)?;
            let intent = current
                .container_launches
                .get(&observed.execution)
                .cloned()
                .ok_or_else(|| {
                    BackendError::invalid("observed container lacks its durable prelaunch intent")
                })?;
            if current
                .cleanup_evidence
                .get(&observed.execution)
                .is_some_and(|reference| {
                    cleanup::stopped(&current, &observed.run, reference).unwrap_or(false)
                })
            {
                return Err(BackendError::unknown(
                    "stopped execution cannot acquire a late container binding",
                ));
            }
            if !matches(&intent, &observed) {
                return Err(BackendError::invalid(
                    "observed container differs from exact prelaunch name/labels/launcher",
                ));
            }
            if let Some(existing) = current
                .runtime_ownerships
                .iter()
                .find(|owned| owned.execution == observed.execution)
            {
                if existing != &observed {
                    return Err(BackendError::invalid("observed ID/scope is immutable"));
                }
            } else {
                if current.terminal.is_some()
                    || observed.container.as_ref().is_some_and(|container| {
                        current.runtime_ownerships.iter().any(|owned| {
                            owned
                                .container
                                .as_ref()
                                .is_some_and(|existing| existing.id == container.id)
                        })
                    })
                {
                    return Err(BackendError::unknown(
                        "container binding conflicts with committed ownership",
                    ));
                }
                current.runtime_ownerships.push(observed.clone());
                save(dir, &current)?;
            }
            // Observation remains retainable after cancel for cleanup; it never authorizes payload.
            Ok(ContainerLaunchBinding {
                intent,
                ownership: observed,
                payload_allowed: !current.cancel_requested && current.terminal.is_none(),
            })
        })
    }
}
