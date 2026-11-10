//! Interpret the allocated job's native R graph. Python transports process facts only.
use super::*;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
struct NativeAllocation {
    version: u32,
    execution: NativeExecution,
    scheduling: Scheduling,
}
#[derive(Deserialize)]
struct NativeExecution {
    #[serde(rename = "R_lite")]
    ranks: Vec<resources::Rank>,
    nodelist: Vec<String>,
}
#[derive(Deserialize)]
struct Scheduling {
    graph: Graph,
}
#[derive(Deserialize)]
struct Graph {
    nodes: Vec<Node>,
}
#[derive(Deserialize)]
struct Node {
    metadata: Metadata,
}
#[derive(Deserialize)]
struct Metadata {
    #[serde(rename = "type")]
    kind: String,
    rank: i64,
    id: u32,
    name: Option<String>,
    #[serde(default)]
    properties: BTreeMap<String, String>,
}

impl Flux {
    fn live_allocation(&self, record: &Record, run: &BackendId) -> Result<Vec<u8>> {
        if record
            .binding
            .as_ref()
            .is_none_or(|binding| binding.id != *run)
            || record.cancel_requested
            || record.terminal.is_some()
            || !record.dispatched
        {
            return Err(BackendError::invalid(
                "allocation observation requires the exact live binding",
            ));
        }
        let reply = self.capture(&["job", "info", &run.job, "R"])?;
        let name = format!("allocation-observation-{}", digest(&reply.stdout));
        blob(
            &record.intent.output_dir.join(format!("{name}.stdout")),
            &reply.stdout,
        )?;
        blob(
            &record.intent.output_dir.join(format!("{name}.stderr")),
            safe(&String::from_utf8_lossy(&reply.stderr))?.as_bytes(),
        )?;
        blob(
            &record.intent.output_dir.join(format!("{name}.status.json")),
            &serde_json::to_vec(&reply.status)?,
        )?;
        if !reply.success {
            return Err(BackendError::unknown(
                "fresh native job R unavailable; raw CLI output/status retained",
            ));
        }
        Ok(reply.stdout)
    }

    pub fn observe_allocation(
        &self,
        owner: &Owner,
        observed: &AllocationObservation,
    ) -> Result<AllocationInput> {
        let record = self.journal.load(owner)?;
        self.instance(&record)?;
        if observed.request_digest != record.intent.request_digest {
            return Err(BackendError::invalid(
                "process observations differ from retained request",
            ));
        }
        // Acceptance-unknown submit: the job may start before attach recovers its binding.
        // Recover the exact original-jobspec match as attach does, durably, before the guard.
        if record.binding.is_none()
            && record.dispatched
            && !record.cancel_requested
            && record.terminal.is_none()
        {
            self.recover(owner)?;
        }
        let record = self.journal.load(owner)?;
        let raw = self.live_allocation(&record, &observed.run)?;
        observed_allocation(&record, observed, &raw)
    }

    pub(super) fn check_live_allocation(
        &self,
        record: &Record,
        facts: &AllocationInput,
    ) -> Result<()> {
        let raw = self.live_allocation(record, &facts.run)?;
        let observed = observed_allocation(
            record,
            &AllocationObservation {
                run: facts.run.clone(),
                request_digest: facts.request_digest.clone(),
                hostname: facts.host.host.clone(),
                allowed_cpus: facts.host.allowed_cpus.clone(),
                cgroup: facts.host.cgroup.clone(),
            },
            &raw,
        )?;
        let supplied: TopologyMapping =
            serde_json::from_slice(&checked_blob(&facts.topology, &record.intent.output_dir)?)?;
        let authoritative: TopologyMapping = serde_json::from_slice(&checked_blob(
            &observed.topology,
            &record.intent.output_dir,
        )?)?;
        if facts.host != observed.host || supplied != authoritative {
            return Err(BackendError::invalid(
                "supplied topology differs from fresh native graph",
            ));
        }
        Ok(())
    }
}

fn observed_allocation(
    record: &Record,
    observed: &AllocationObservation,
    raw: &[u8],
) -> Result<AllocationInput> {
    let native: NativeAllocation = serde_json::from_slice(raw)?;
    if native.version != 1
        || native.execution.ranks.len() != 1
        || native.execution.nodelist != [observed.hostname.clone()]
        || native.scheduling.graph.nodes.len() > 65536
    {
        return Err(BackendError::invalid(
            "single-host native allocation missing or ambiguous",
        ));
    }
    let rank = &native.execution.ranks[0];
    let ranks = resources::idset(&rank.rank)?;
    if ranks != BTreeSet::from([0]) || observed.hostname != "dl380g10" {
        return Err(BackendError::invalid(
            "runtime host differs from first-cut placement",
        ));
    }
    let cores = resources::idset(&rank.children.core)?;
    let nodes = &native.scheduling.graph.nodes;
    let hosts: Vec<_> = nodes
        .iter()
        .filter(|node| node.metadata.kind == "node")
        .collect();
    if hosts.len() != 1
        || hosts[0].metadata.rank != 0
        || hosts[0].metadata.name.as_deref() != Some(observed.hostname.as_str())
    {
        return Err(BackendError::invalid(
            "native graph host/rank differs from allocated nodelist",
        ));
    }
    let mut mapped = BTreeSet::new();
    let mut mappings = Vec::new();
    for node in nodes.iter().filter(|node| node.metadata.kind == "core") {
        let core = &node.metadata;
        if core.rank != 0 || !cores.contains(&core.id) || !mapped.insert(core.id) {
            return Err(BackendError::invalid(
                "native graph core allocation missing, changed or ambiguous",
            ));
        }
        let os_pus = core
            .properties
            .get("os-pus")
            .ok_or_else(|| BackendError::invalid("native graph lacks authoritative os-pus"))?;
        mappings.push(CoreMapping {
            flux_core: core.id,
            os_pus: os_pus.clone(),
        });
    }
    if mapped != cores {
        return Err(BackendError::invalid(
            "native graph does not cover the exact core allocation",
        ));
    }
    mappings.sort_by_key(|core| core.flux_core);
    let topology = TopologyMapping {
        run: observed.run.clone(),
        request_digest: observed.request_digest.clone(),
        ranks: vec![RankMapping {
            rank: 0,
            host: observed.hostname.clone(),
            cores: mappings,
        }],
    };
    let topology = serde_json::to_vec(&topology)?;
    Ok(AllocationInput {
        run: observed.run.clone(),
        request_digest: observed.request_digest.clone(),
        allocation: blob(
            &record
                .intent
                .output_dir
                .join(format!("allocation-{}.json", digest(raw))),
            raw,
        )?,
        topology: blob(
            &record
                .intent
                .output_dir
                .join(format!("topology-{}.json", digest(&topology))),
            &topology,
        )?,
        host: ProcessPlacement {
            rank: 0,
            host: observed.hostname.clone(),
            allowed_cpus: observed.allowed_cpus.clone(),
            cgroup: observed.cgroup.clone(),
        },
    })
}
