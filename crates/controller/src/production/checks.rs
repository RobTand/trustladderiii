use super::*;
use crate::{Error, Ownership, executor::Registry};
use std::{path::Path, time::Duration};
use tl3_prismabuild::{Demand, Options, PrismaBuild, Tool};

/// Preserve the controller routing tag while selecting its observation source.
pub enum CheckSource {
    PrismaBuild(Box<adapters::prismabuild::Source>),
    Flux {
        daytime: adapters::backend_watch::Source,
        workers: adapters::backend_watch::Source,
    },
}
impl CheckSource {
    pub fn register(self, sources: &mut crate::ingress::Sources) {
        match self {
            Self::PrismaBuild(source) => sources.register(*source),
            Self::Flux { daytime, workers } => {
                sources.register(daytime);
                sources.register(workers);
            }
        }
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
    if let tl3_config::SelectedExecution::Flux(flux) = config.selected_execution()? {
        require_drained_pb(&records.join("prismabuild"))?;
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
        let source = adapter.source()?;
        registry.register(crate::executor::Upstream::PrismaBuild, adapter);
        return Ok(CheckSource::Flux {
            daytime: source,
            workers,
        });
    }
    let tools = &config.prismabuild.tools_dir;
    let tool = Tool {
        program: "python3".into(),
        args: vec![tools.join("pbrun.py").into()],
    };
    let queue = tools
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or(Error::Invalid(
            "PrismaBuild tools need an installed fleet root",
        ))?
        .join("pb-queue");
    let client = PrismaBuild::new(Options {
        queue_root: queue,
        tool,
        bell_url: "http://127.0.0.1:9470/".into(),
    })
    .map_err(|e| Error::Adapter(Box::new(e)))?;
    let app_root = config
        .workers
        .python
        .ancestors()
        .nth(4)
        .ok_or(Error::Invalid(
            "workers.python needs the installed venv layout",
        ))?;
    let pb = adapters::prismabuild::Adapter::new(
        client,
        adapters::prismabuild::Settings {
            records: records.join("prismabuild"),
            harness: app_root.join("harness/src/tl3_harness"),
            python: vec![config.workers.python.clone().into_os_string()],
            image: config.prismabuild.container_image.clone(),
            demand: Demand {
                cpus: 1.try_into().expect("positive demand"),
                mem_gb: 2,
                gpu: false,
            },
            quiet: Duration::from_secs(u64::from(config.allowances.prismabuild.no_progress_secs)),
            priority: 0,
            result_tool: Tool {
                program: "python3".into(),
                args: vec![tools.join("pbwait.py").into()],
            },
        },
        ownership,
    )?;
    let pb = pb
        .with_repositories(&config.github.repos)
        .with_protected_paths(&config.planning.protected_paths);
    let source = pb.source();
    registry.register(crate::executor::Upstream::PrismaBuild, pb);
    Ok(CheckSource::PrismaBuild(Box::new(source)))
}
