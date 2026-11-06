use super::*;
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Deserialize)]
pub(super) struct ResourceSet { pub version: u32, pub execution: Execution }
#[derive(Deserialize)]
pub(super) struct Execution {
    #[serde(rename = "R_lite")]
    pub ranks: Vec<Rank>,
}
#[derive(Deserialize)]
pub(super) struct Rank { pub rank: String, pub children: Children }
#[derive(Deserialize)]
pub(super) struct Children { pub core: String, #[serde(default)] pub gpu: String }

pub(super) fn idset(text: &str) -> Result<BTreeSet<u32>> {
    let mut ids = BTreeSet::new();
    if text.is_empty() { return Ok(ids); }
    if text.len() > 65_536 { return Err(BackendError::invalid("ID set exceeds bound")); }
    let text = text.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(text);
    for part in text.split(',') {
        let (first, last) = part.split_once('-').unwrap_or((part, part));
        let first: u32 = first.parse().map_err(|_| BackendError::invalid("invalid native ID set"))?;
        let last: u32 = last.parse().map_err(|_| BackendError::invalid("invalid native ID set"))?;
        if first > last || last > 65_535 || last - first > 65_535 { return Err(BackendError::invalid("invalid native ID range")); }
        for id in first..=last {
            if !ids.insert(id) { return Err(BackendError::invalid("overlapping native ID ranges")); }
        }
    }
    Ok(ids)
}

fn counts(resource: &ResourceSet) -> Result<Vec<(u32, u64, u64)>> {
    if resource.version != 1 || resource.execution.ranks.len() > 4096 { return Err(BackendError::invalid("unsupported native R version/shape")); }
    let mut counts = Vec::new();
    let mut seen = BTreeSet::new();
    for ranks in &resource.execution.ranks {
        let cpus = idset(&ranks.children.core)?.len() as u64;
        let gpus = idset(&ranks.children.gpu)?.len() as u64;
        for rank in idset(&ranks.rank)? {
            if !seen.insert(rank) { return Err(BackendError::invalid("duplicate native rank")); }
            counts.push((rank, cpus, gpus));
        }
    }
    Ok(counts)
}

impl Flux {
    pub(super) fn probe_resources(&self, reservation: &Reservation, placement: &Placement) -> Result<Probe> {
        validate::reservation(reservation)?;
        let mut probe = Probe { availability: Availability::Unknown, reason: "native resource query unavailable".into(),
            total_cpus: 0, free_cpus: 0, total_gpus: 0, free_gpus: 0, memory_guarantee: MemoryGuarantee::DeclaredOnly };
        if validate::placement(placement).is_err() || reservation.memory_guarantee == MemoryGuarantee::RequireAdmission || reservation.gpus > 0 {
            probe.availability = Availability::Incompatible;
            probe.reason = "required memory admission or GPU P-core/CDI capability is unqualified".into();
            return Ok(probe);
        }
        let parsed = (|| -> Result<_> {
            let all = self.capture(&["resource", "R", "--states=all"])?;
            let free = self.capture(&["resource", "R", "--states=free"])?;
            if !all.success || !free.success { return Err(BackendError::unknown("resource CLI failed")); }
            // The current qualified image dependency is rank zero/dl380g10 only.
            let all = counts(&serde_json::from_slice(&all.stdout)?)?.into_iter().filter(|(rank, _, _)| *rank == 0).collect::<Vec<_>>();
            let free = counts(&serde_json::from_slice(&free.stdout)?)?.into_iter().filter(|(rank, _, _)| *rank == 0).collect::<Vec<_>>();
            for (rank, cpus, gpus) in &free {
                if !all.iter().any(|(r, c, g)| r == rank && c >= cpus && g >= gpus) {
                    return Err(BackendError::unknown("free resource view contradicts total view"));
                }
            }
            Ok((all, free))
        })();
        let Ok((all, free)) = parsed else { return Ok(probe); };
        probe.total_cpus = all.iter().map(|(_, cpus, _)| cpus).sum();
        probe.total_gpus = all.iter().map(|(_, _, gpus)| gpus).sum();
        probe.free_cpus = free.iter().map(|(_, cpus, _)| cpus).sum();
        probe.free_gpus = free.iter().map(|(_, _, gpus)| gpus).sum();
        let fits = |counts: &[(u32, u64, u64)]| counts.iter().any(|(_, cpus, gpus)| *cpus >= u64::from(reservation.cpus.get()) && *gpus >= u64::from(reservation.gpus));
        if !fits(&all) {
            probe.availability = Availability::Incompatible;
            probe.reason = "no native rank can accommodate the single-task CPU/GPU shape".into();
        } else if !fits(&free) {
            probe.availability = Availability::Busy;
            probe.reason = "native instance healthy; compatible capacity currently occupied".into();
        } else {
            probe.availability = Availability::Available;
            probe.reason = "native CPU/GPU capacity visible; not admission or runtime qualification".into();
        }
        Ok(probe)
    }
}
