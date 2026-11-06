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
    pub call: String,
}

impl WaitBinding {
    pub fn key(&self) -> Result<String> {
        if !backend::hex(&self.call, 64) { return Err(Error::Invalid("worker call needs its retained fresh identity")); }
        Ok(format!("worker:{}", durable::digest(&serde_json::to_vec(&(&self.profile, &self.input, &self.served_tree, &self.call))?)))
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
        if let Some(binding) = &retained.binding {
            binding.id.validate().map_err(backend_error)?;
            if binding.intent != self.intent || binding.id.uri != self.intent.uri || binding.id.instance != self.intent.instance {
                return Err(Error::Invalid("durable worker native binding changed"));
            }
        }
        if retained.intent != self.intent || retained.request.tree != self.input
            || !matches!(retained.request.suite, backend::Suite::WorkerTests { .. } | backend::Suite::Generator { .. })
            || retained.intent.request_digest != durable::digest(&serde_json::to_vec(&retained.request)?)
            || self.binding.as_ref().is_some_and(|binding| retained.binding.as_ref() != Some(binding))
        { return Err(Error::Invalid("worker wait differs from durable backend request")); }
        Ok(profile)
    }
}

fn backend_error(error: backend::BackendError) -> Error { Error::Adapter(Box::new(error)) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_wait_rejects_other_attempt_checkout_generation_and_unrelated_suite() {
        let repo = crate::git_tests::Repo::new();
        let root = repo.root.path();
        let attempt = root.join("attempt-00000000");
        fs::create_dir(&attempt).unwrap();
        let flux = tl3_config::Flux {
            instance_generation: "fixture/00000000000000000000000000000000".into(),
            executable: "/fixture/flux".into(), backend_cli: "/fixture/tl3-run-backend".into(),
            container_mode: tl3_config::ContainerMode::OwnScope, runtime_bundles: vec![],
            uri: "local:///fixture".into(), environment: BTreeMap::new(),
            staging_root: root.join("staging"), output_root: root.join("output"),
            container_image: format!("sha256:{}", "a".repeat(64)),
        };
        let selected = Selected::Flux(flux.clone());
        let mut worker = json!({"worktree": repo.path, "inputs": {"tests": selected.inputs(&flux.container_image,
            tl3_config::TestDemand::default(), &attempt.join("execution.json"))}});
        selected.bind_attempt(&mut worker, &attempt, &repo.delivery()).unwrap();
        write(&attempt.join("request.json"), &worker).unwrap();
        let options = crate::production::flux_options(&flux);
        let tree = backend::TreeIdentity { head: "a".repeat(40), tree: "b".repeat(40), input_digest: "c".repeat(64),
            root: flux.staging_root.join("immutable-call"), manifest: flux.staging_root.join("input-manifest.json") };
        let blob = backend::BlobRef { path: root.join("blob"), sha256: "d".repeat(64), bytes: 1 };
        let request = backend::Request {
            repo: "owner/sandbox".into(), tree: tree.clone(), suite: backend::Suite::WorkerTests { input_digest: "e".repeat(64) },
            reservation: backend::Reservation { cpus: 2.try_into().unwrap(), gpus: 0, memory_bytes: 4.try_into().unwrap(),
                memory_guarantee: backend::MemoryGuarantee::DeclaredOnly, parallelism: 2.try_into().unwrap(), native_threads: 1.try_into().unwrap() },
            runner: backend::RunnerIdentity { executable: root.join("collector"), executable_digest: "f".repeat(64), arguments: vec![],
                manifest: root.join("runner.json"), manifest_digest: "f".repeat(64) },
            runtime: backend::RuntimeIdentity { manifest: root.join("runtime.json"), manifest_digest: "f".repeat(64),
                runtime: backend::ExecutionRuntime::Native { interpreter: "/fixture/python".into() }, architecture: "x86_64".into(),
                expected_image: None, preflight: blob, host_dependency: None, image: None, cleanup: None },
            environment: BTreeMap::new(), scratch_root: root.join("scratch"), no_progress_seconds: None, walltime_seconds: 60.try_into().unwrap(),
        };
        let mut wait = WaitBinding { profile: attempt.join("execution.json"), owner: backend::Owner { owner: "worker-tools".into(), correlation: String::new() },
            intent: backend::SubmissionIntent { backend: backend::BackendTag::Flux, instance: options.instance_generation.clone(), uri: options.uri.clone(),
                owner: "worker-tools".into(), correlation: String::new(), request_digest: durable::digest(&serde_json::to_vec(&request).unwrap()), output_dir: root.join("output") },
            binding: None, input: tree, served_tree: "1".repeat(40), call: "2".repeat(64) };
        let key = wait.key().unwrap();
        wait.owner.correlation = key.replace("worker:", "worker-");
        wait.intent.correlation = wait.owner.correlation.clone();
        let journal = durable::Journal::new(options.journal_root.clone()).unwrap();
        let mut record = durable::Record { request, intent: wait.intent.clone(), dispatched: true, binding: None, cancel_requested: false,
            events: vec![], terminal: None, runtime_ownerships: vec![], cleanup_evidence: BTreeMap::new(), jobspec: None, allocation_grant: None };
        journal.transaction(&wait.owner, |dir, _| durable::save(dir, &record)).unwrap();
        assert!(wait.validate(&key, &repo.path, &options).is_ok());
        let mut changed = options.clone(); changed.instance_generation = "replacement".into();
        assert!(wait.validate(&key, &repo.path, &changed).is_err());
        assert!(wait.validate(&key, root, &options).is_err());
        assert!(wait.validate("fABC", &repo.path, &options).is_err());
        let mut repeated = wait.clone(); repeated.call = "3".repeat(64);
        assert_ne!(wait.key().unwrap(), repeated.key().unwrap(), "intentional repeats need separate references");
        record.binding = Some(backend::RunBinding { intent: wait.intent.clone(), id: backend::BackendId {
            backend: backend::BackendTag::Flux, uri: options.uri.clone(), instance: "another-generation".into(), job: "fABC".into() } });
        journal.transaction(&wait.owner, |dir, _| durable::save(dir, &record)).unwrap();
        assert!(wait.validate(&key, &repo.path, &options).is_err());
        record.binding = None;
        record.request.suite = backend::Suite::Full { membership_digest: "4".repeat(64) };
        journal.transaction(&wait.owner, |dir, _| durable::save(dir, &record)).unwrap();
        assert!(wait.validate(&key, &repo.path, &options).is_err());
        worker["inputs"]["tests"]["execution"]["profile"] = json!(root.join("another-attempt/execution.json"));
        write(&attempt.join("request.json"), &worker).unwrap();
        assert!(wait.validate(&key, &repo.path, &options).is_err());
    }
}
