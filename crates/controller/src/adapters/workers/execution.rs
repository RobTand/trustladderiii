//! Selected execution materialization for every worker tool caller.
use super::*;
use crate::run_backend::{self as backend, durable};
use serde::{Deserialize, Serialize};
mod waits;
pub use waits::Waits;

#[derive(Clone)]
pub(super) enum Selected {
    PrismaBuild(tl3_config::PrismaBuild),
    Flux(tl3_config::Flux),
}

impl Selected {
    pub(super) fn new(config: &tl3_config::Config) -> Result<Self> {
        Ok(match config.selected_execution()? {
            tl3_config::SelectedExecution::PrismaBuild(pb) => Self::PrismaBuild(pb.clone()),
            tl3_config::SelectedExecution::Flux(flux) => Self::Flux(flux.clone()),
        })
    }

    pub(super) fn name(&self) -> &'static str {
        match self { Self::PrismaBuild(_) => "prismabuild", Self::Flux(_) => "flux" }
    }

    pub(super) fn image(&self) -> &str {
        match self {
            Self::PrismaBuild(pb) => &pb.container_image,
            Self::Flux(flux) => &flux.container_image,
        }
    }

    pub(super) fn inputs(&self, image: &str, demand: tl3_config::TestDemand, profile: &Path) -> Value {
        let mut value = json!({"image": image, "cpus": demand.cpus, "mem_gb": demand.mem_gb});
        match self {
            Self::PrismaBuild(pb) => value["tools_path"] = json!(pb.tools_dir),
            Self::Flux(_) => value["execution"] = json!({"backend": "flux", "profile": profile}),
        }
        value
    }

    pub(super) fn bind_attempt(&self, request: &mut Value, attempt: &Path, delivery: &Delivery) -> Result<()> {
        let Self::Flux(flux) = self else { return Ok(()); };
        if request["inputs"].get("tests").is_none() { return Ok(()); }
        let options = crate::production::flux_options(flux);
        let profile = Profile {
            schema: "tl3.worker_execution.v1".into(), attempt: fs::canonicalize(attempt)?,
            checkout: fs::canonicalize(request["worktree"].as_str().ok_or(Error::Invalid("worker checkout missing"))?)?,
            delivery: delivery.clone(), options: options.bootstrap.config,
            job_environment: options.journal_root.join("job-environment.json"),
            staging_root: flux.staging_root.clone(), runtime_bundles: flux.runtime_bundles.clone(),
            container_mode: flux.container_mode,
        };
        let path = profile.attempt.join("execution.json");
        private_create(&path, &serde_json::to_vec(&profile)?)?;
        request["inputs"]["tests"]["execution"]["profile"] = json!(path);
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub schema: String,
    pub attempt: PathBuf,
    pub checkout: PathBuf,
    pub delivery: Delivery,
    pub options: PathBuf,
    pub job_environment: PathBuf,
    pub staging_root: PathBuf,
    pub runtime_bundles: Vec<PathBuf>,
    pub container_mode: tl3_config::ContainerMode,
}

fn private_create(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    File::open(path.parent().ok_or(Error::Invalid("worker profile needs parent"))?)?.sync_all()?;
    Ok(())
}

/// An opaque worker reference binds an attempt, checkout and shared backend intent.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitBinding {
    pub profile: PathBuf,
    pub owner: backend::Owner,
    pub intent: backend::SubmissionIntent,
    pub binding: Option<backend::RunBinding>,
    pub input: backend::TreeIdentity,
    pub served_tree: String,
}

impl WaitBinding {
    pub fn key(&self) -> Result<String> {
        Ok(format!("worker:{}", durable::digest(&serde_json::to_vec(&(&self.profile, &self.input, &self.served_tree))?)))
    }

    pub fn validate(&self, key: &str, checkout: &Path, options: &backend::Options) -> Result<Profile> {
        let profile: Profile = serde_json::from_slice(&durable::read(&self.profile).map_err(backend_error)?)?;
        if self.key()? != key || self.owner.owner != "worker-tools"
            || self.owner.correlation != key.replace("worker:", "worker-")
            || self.intent.owner != self.owner.owner || self.intent.correlation != self.owner.correlation
            || self.intent.uri != options.uri || self.intent.instance != options.instance_generation
            || profile.schema != "tl3.worker_execution.v1" || profile.checkout != fs::canonicalize(checkout)?
            || self.profile != profile.attempt.join("execution.json")
            || profile.options != options.bootstrap.config
            || profile.job_environment != options.journal_root.join("job-environment.json")
            || !backend::hex(&self.served_tree, 40)
        { return Err(Error::Invalid("worker wait does not match its retained attempt and selected backend")); }
        let request = read(&profile.attempt.join("request.json"))?.ok_or(Error::Invalid("worker wait attempt request missing"))?;
        if request["inputs"]["tests"]["execution"]["profile"] != json!(self.profile)
            || request["worktree"].as_str().map(Path::new).map(fs::canonicalize).transpose()?.as_ref() != Some(&profile.checkout)
        { return Err(Error::Invalid("worker wait belongs to another attempt checkout")); }
        if let Some(binding) = &self.binding {
            binding.id.validate().map_err(backend_error)?;
            if binding.intent != self.intent || binding.id.uri != self.intent.uri || binding.id.instance != self.intent.instance {
                return Err(Error::Invalid("worker wait native binding changed"));
            }
        }
        let retained = durable::Journal::new(options.journal_root.clone()).map_err(backend_error)?.load(&self.owner).map_err(backend_error)?;
        if retained.intent != self.intent || retained.request.tree != self.input
            || !matches!(retained.request.suite, backend::Suite::WorkerTests { .. } | backend::Suite::Generator { .. })
            || self.binding.as_ref().is_some_and(|binding| retained.binding.as_ref() != Some(binding))
        { return Err(Error::Invalid("worker wait differs from durable backend request")); }
        Ok(profile)
    }
}

fn backend_error(error: backend::BackendError) -> Error { Error::Adapter(Box::new(error)) }
