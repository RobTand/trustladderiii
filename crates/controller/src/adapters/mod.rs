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

pub use record::delivery_counts;
