//! Offline deployment binding to the completed, source-owned FX qualification.
use crate::run_backend::{self as backend, durable};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use tl3_config::{ContainerMode, FluxActivation};

const PIN: &[u8] = include_bytes!("activation/qualification.json");

#[derive(Deserialize)]
struct Qualification {
    metadata: Metadata,
    files: Vec<backend::BundleFile>,
    profile_files: Vec<PathBuf>,
}

#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Metadata {
    schema: String,
    owner: String,
    uid: u32,
    interpreter: PathBuf,
    podman: PathBuf,
    container_interpreter: PathBuf,
    cleanup_arguments: Vec<String>,
    collector: PathBuf,
    cleanup: PathBuf,
    trusted_root: PathBuf,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceScope {
    PlannedFreshInit,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupRequirement {
    Verified,
}

/// Identities and hashes only. This is a plan, never evidence of a completed new-profile run.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileBinding {
    pub manifest: PathBuf,
    pub manifest_sha256: String,
    pub image: backend::ImageIdentity,
    pub preflight: backend::BlobRef,
    pub required_native_success_runs: std::num::NonZeroU32,
    pub required_cleanup: CleanupRequirement,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentRecord {
    pub schema: String,
    pub qualification_sha256: String,
    pub producer_commit: String,
    pub backend_options_sha256: String,
    pub backend_cli_sha256: String,
    pub executable_sha256: String,
    pub staging_root_sha256: String,
    pub container_mode: ContainerMode,
    pub acceptance_scope: AcceptanceScope,
    pub profiles: Vec<ProfileBinding>,
}

pub struct RuntimeActivation {
    record: DeploymentRecord,
    qualification: Qualification,
}

fn invalid(field: &'static str) -> Error {
    Error::Invalid(field)
}

/// Stable serde identities contain no ambient environment. Diagnostics never include these bytes.
pub fn identity<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_vec(value)
        .map(|bytes| durable::digest(&bytes))
        .map_err(|_| invalid("flux.activation.identity"))
}

/// Deployment policy and tool access stay bound; fleet identity is observed at execution.
pub fn options_identity(options: &backend::Options) -> Result<String> {
    let mut value =
        serde_json::to_value(options).map_err(|_| invalid("flux.activation.identity"))?;
    let fields = value
        .as_object_mut()
        .ok_or_else(|| invalid("flux.activation.identity"))?;
    if let Some(environment) = fields
        .get_mut("environment")
        .and_then(serde_json::Value::as_object_mut)
    {
        environment.remove("FLUX_URI");
        environment.remove("FLUX_VANILLA_RUN");
    }
    identity(&value)
}

pub fn qualification_sha256() -> String {
    durable::digest(PIN)
}

/// Hash the provisioned CLI itself, without executing it or limiting debug binaries to JSON bounds.
pub fn executable_sha256(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|_| invalid("flux.activation.cli"))?;
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err(invalid("flux.activation.cli"));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| invalid("flux.activation.cli"))?;
        if count == 0 {
            return Ok(format!("{:x}", hash.finalize()));
        }
        hash.update(&buffer[..count]);
    }
}

fn registration_reservation() -> backend::Reservation {
    backend::Reservation {
        cpus: 1.try_into().expect("positive registration CPU"),
        gpus: 0,
        memory_bytes: (1u64 << 30)
            .try_into()
            .expect("positive registration memory"),
        memory_guarantee: backend::MemoryGuarantee::DeclaredOnly,
        parallelism: 1.try_into().expect("positive registration parallelism"),
        native_threads: 1.try_into().expect("positive registration thread bound"),
    }
}

impl RuntimeActivation {
    pub fn load(config: &tl3_config::Flux) -> Result<Self> {
        let activation = Self::load_bound(
            config.activation.as_ref(),
            &config.runtime_bundles,
            config.container_mode,
            &config.staging_root,
            &crate::production::flux_options(config),
        )?;
        activation.validate_image(&config.container_image)?;
        Ok(activation)
    }

    /// Workers use retained deployment identity plus their actual effective options file.
    pub fn load_bound(
        pin: Option<&FluxActivation>,
        bundles: &[PathBuf],
        mode: ContainerMode,
        staging_root: &Path,
        options: &backend::Options,
    ) -> Result<Self> {
        let pin = pin.ok_or_else(|| invalid("flux.activation"))?;
        if !backend::absolute(&pin.record) || !backend::hex(&pin.sha256, 64) {
            return Err(invalid("flux.activation.record"));
        }
        let raw = durable::read(&pin.record).map_err(|_| invalid("flux.activation.record"))?;
        if durable::digest(&raw) != pin.sha256 {
            return Err(invalid("flux.activation.sha256"));
        }
        let record: DeploymentRecord =
            serde_json::from_slice(&raw).map_err(|_| invalid("flux.activation.record"))?;
        if record.schema != "tl3.fx_activation.v2" {
            return Err(invalid("flux.activation.schema"));
        }
        if record.qualification_sha256 != qualification_sha256() {
            return Err(invalid("flux.activation.qualification_sha256"));
        }
        if !backend::hex(&record.producer_commit, 40) {
            return Err(invalid("flux.activation.producer_commit"));
        }
        if record.container_mode != mode {
            return Err(invalid("flux.activation.container_mode"));
        }
        if record.staging_root_sha256 != identity(&staging_root)? {
            return Err(invalid("flux.activation.staging_root_sha256"));
        }
        let configured: BTreeSet<_> = bundles.iter().collect();
        let bound: BTreeSet<_> = record
            .profiles
            .iter()
            .map(|profile| &profile.manifest)
            .collect();
        let profiles: BTreeSet<_> = record
            .profiles
            .iter()
            .map(|profile| &profile.image.profile)
            .collect();
        if configured.is_empty()
            || configured.len() != bundles.len()
            || bound.len() != record.profiles.len()
            || configured != bound
            || profiles.len() != record.profiles.len()
        {
            return Err(invalid("flux.activation.profiles"));
        }
        let activation = Self {
            record,
            qualification: serde_json::from_slice(PIN)
                .map_err(|_| invalid("flux.activation.qualification"))?,
        };
        activation.validate_options(options)?;
        for profile in &activation.record.profiles {
            activation.checked_bundle(profile, &registration_reservation(), vec![])?;
        }
        Ok(activation)
    }

    pub fn validate_options(&self, options: &backend::Options) -> Result<()> {
        for (actual, expected, field) in [
            (
                options_identity(options)?,
                &self.record.backend_options_sha256,
                "flux.activation.backend_options_sha256",
            ),
            (
                executable_sha256(&options.bootstrap.cli)?,
                &self.record.backend_cli_sha256,
                "flux.activation.backend_cli_sha256",
            ),
            (
                executable_sha256(&options.executable)?,
                &self.record.executable_sha256,
                "flux.activation.executable_sha256",
            ),
        ] {
            if actual != *expected {
                return Err(invalid(field));
            }
        }
        Ok(())
    }

    pub fn validate_image(&self, image: &str) -> Result<()> {
        if !tl3_config::immutable_image(image)
            || !self.record.profiles.iter().any(|profile| {
                image == profile.image.reference || image == profile.image.config_digest
            })
        {
            return Err(invalid("flux.activation.image"));
        }
        Ok(())
    }

    fn checked_bundle(
        &self,
        profile: &ProfileBinding,
        reservation: &backend::Reservation,
        arguments: Vec<String>,
    ) -> Result<backend::PreparedRuntime> {
        let (manifest, reference) = backend::prepared::read_bundle(&profile.manifest)
            .map_err(|_| invalid("flux.activation.bundle"))?;
        if reference.sha256 != profile.manifest_sha256 {
            return Err(invalid("flux.activation.manifest_sha256"));
        }
        if manifest.producer_commit != self.record.producer_commit {
            return Err(invalid("flux.activation.producer_commit"));
        }
        let metadata: Metadata = serde_json::from_value({
            let mut value =
                serde_json::to_value(&manifest).map_err(|_| invalid("flux.activation.metadata"))?;
            let object = value
                .as_object_mut()
                .ok_or_else(|| invalid("flux.activation.metadata"))?;
            object.remove("producer_commit");
            object.remove("files");
            value
        })
        .map_err(|_| invalid("flux.activation.metadata"))?;
        if metadata != self.qualification.metadata {
            return Err(invalid("flux.activation.metadata"));
        }
        let fixed: BTreeMap<_, _> = self
            .qualification
            .files
            .iter()
            .map(|file| (&file.path, file))
            .collect();
        let actual: BTreeMap<_, _> = manifest
            .files
            .iter()
            .map(|file| (&file.path, file))
            .collect();
        let mut paths: BTreeSet<_> = fixed.keys().copied().collect();
        paths.extend(&self.qualification.profile_files);
        if actual.keys().copied().collect::<BTreeSet<_>>() != paths
            || fixed
                .iter()
                .any(|(path, file)| actual.get(path) != Some(file))
        {
            return Err(invalid("flux.activation.code_inventory"));
        }
        let prepared =
            backend::prepared::prepare_runtime(&profile.manifest, reservation, arguments)
                .map_err(|_| invalid("flux.activation.bundle"))?;
        crate::adapters::run_backend::flux::validate_prepared_runtime(&prepared)
            .map_err(|_| invalid("flux.activation.preflight"))?;
        if prepared.runtime.image.as_ref() != Some(&profile.image)
            || prepared.runtime.preflight != profile.preflight
            || !tl3_config::immutable_image(&profile.image.reference)
            || !matches!(&prepared.runtime.runtime, backend::ExecutionRuntime::RootlessPodman { mode, .. } if *mode == self.record.container_mode)
        {
            return Err(invalid("flux.activation.profile"));
        }
        Ok(prepared)
    }

    /// Re-read the selected sealed bundle immediately before constructing a request.
    pub fn validate_prepared(
        &self,
        prepared: &backend::PreparedRuntime,
        reservation: &backend::Reservation,
    ) -> Result<()> {
        let profile = self
            .record
            .profiles
            .iter()
            .find(|profile| profile.manifest == prepared.bundle.path)
            .ok_or_else(|| invalid("flux.activation.profile"))?;
        let canonical =
            self.checked_bundle(profile, reservation, prepared.runner.arguments.clone())?;
        if serde_json::to_value(&canonical).map_err(|_| invalid("flux.activation.runtime"))?
            != serde_json::to_value(prepared).map_err(|_| invalid("flux.activation.runtime"))?
        {
            return Err(invalid("flux.activation.runtime"));
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests;
