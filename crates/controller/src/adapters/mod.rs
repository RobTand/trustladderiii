pub mod github;
pub mod mergify;
pub mod prismabuild;
mod record;
pub mod workers;
pub mod worktrees;

use crate::{
    Result,
    executor::{Registry, Upstream},
};
use std::path::Path;

pub fn simulation(root: &Path) -> Result<Registry> {
    std::fs::create_dir_all(root)?;
    let mut adapters = Registry::default();
    adapters.register(Upstream::GitHub, github::Fake::new(root));
    adapters.register(Upstream::Mergify, mergify::Fake::new(root));
    adapters.register(Upstream::PrismaBuild, prismabuild::Fake::new(root));
    adapters.register(Upstream::Workers, workers::Fake::new(root));
    adapters.register(Upstream::Worktrees, worktrees::Fake::new(root));
    Ok(adapters)
}

pub use record::{delivered_notes, delivery_counts, delivery_epochs};

/// Real ingest, audit, planning, maker and review processes; other upstreams are simulated.
pub fn simulation_with_workers(
    root: &Path,
    config: &tl3_config::Config,
) -> Result<(Registry, workers::WorkerOutputSource)> {
    let mut registry = simulation(root)?;
    let supervisor = workers::Supervisor::new(config, root)?;
    let source = supervisor.source();
    registry.register(Upstream::Workers, supervisor);
    Ok((registry, source))
}
