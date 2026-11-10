use super::*;
use crate::{Error, Ownership, executor::Registry};
use std::path::Path;

/// The check observation sources: the daytime checks and the worker waits on Flux, the only
/// executor. The controller routing tag stays `Upstream::PrismaBuild`.
pub struct CheckSource {
    daytime: adapters::backend_watch::Source,
    workers: adapters::backend_watch::Source,
}
impl CheckSource {
    pub fn register(self, sources: &mut crate::ingress::Sources) {
        sources.register(self.daytime);
        sources.register(self.workers);
    }
}

/// Compose production execution after acquiring the controller claim. Does not launch work.
/// The daemon's startup activation gate is separate from this construction boundary.
pub fn register_execution(
    config: &tl3_config::Config,
    records: &Path,
    registry: &mut Registry,
    ownership: &mut Ownership,
) -> Result<CheckSource> {
    ownership.ensure_held()?;
    let flux = &config.flux;
    let preparation = adapters::flux_checks::staging::Stager::new(config)?;
    let options = provision_flux_options(flux, ownership)?;
    let backend = std::sync::Arc::new(
        adapters::run_backend::flux::Flux::new(options.clone())
            .map_err(|error| Error::Adapter(Box::new(error)))?,
    );
    let (waits, workers) = adapters::workers::execution::Waits::new(
        backend.clone(),
        options.clone(),
        &records.join("worker-waits"),
    )?;
    let mut adapter = adapters::flux_checks::Adapter::new(
        backend,
        options,
        &records.join("flux-checks"),
        preparation,
        ownership,
    )?
    .with_worker_waits(waits);
    let daytime = adapter.source()?;
    registry.register(crate::executor::Upstream::PrismaBuild, adapter);
    Ok(CheckSource { daytime, workers })
}
