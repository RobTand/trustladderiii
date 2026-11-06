use super::*;
use crate::{Error, Ownership, executor::Registry};
use std::{path::Path, time::Duration};
use tl3_prismabuild::{Demand, Options, PrismaBuild, Tool};

/// Preserve the controller routing tag while selecting its observation source.
pub enum CheckSource {
    PrismaBuild(adapters::prismabuild::Source),
    Flux(adapters::backend_watch::Source),
}
impl crate::ingress::Source for CheckSource {
    fn run(self, sender: crate::sync::mpsc::Sender<crate::cli::Input>) -> Result<()> {
        match self {
            Self::PrismaBuild(source) => source.run(sender),
            Self::Flux(source) => source.run(sender),
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
    Ok(CheckSource::PrismaBuild(source))
}
