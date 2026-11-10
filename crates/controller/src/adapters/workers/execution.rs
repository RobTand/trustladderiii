//! Selected execution materialization for every worker tool caller.
use super::*;
use crate::run_backend::{self as backend, durable};
use serde::{Deserialize, Serialize};
mod inputs;
mod waits;
pub(crate) use inputs::reservation;
pub use inputs::{Command, command, execute};
pub use waits::Waits;

/// The worker tests executor. Flux is the only one. Without a selected Flux executor every
/// run that needs worker tests is refused; nothing is defaulted (PrismaBuild is retired).
#[derive(Clone)]
pub(super) struct Selected(
    Option<Box<tl3_config::Flux>>,
    crate::adapters::images::Images,
);

impl Selected {
    pub(super) fn new(config: &tl3_config::Config) -> Result<Self> {
        Ok(Self(
            match config.selected_execution()? {
                tl3_config::SelectedExecution::Flux(flux) => Some(Box::new(flux.clone())),
                tl3_config::SelectedExecution::PrismaBuild(_) => None,
            },
            crate::adapters::images::Images::new(&config.github.repos),
        ))
    }

    fn flux(&self) -> Result<&tl3_config::Flux> {
        self.0.as_deref().ok_or(Error::Invalid(
            "worker tests need execution_backend = 'flux'; PrismaBuild is retired",
        ))
    }

    pub(super) fn image(&self) -> Result<&str> {
        Ok(&self.flux()?.container_image)
    }

    pub(super) fn inputs(
        &self,
        image: &str,
        demand: tl3_config::TestDemand,
        profile: &Path,
    ) -> Result<Value> {
        self.flux()?;
        Ok(
            json!({"image": image, "cpus": demand.cpus, "mem_gb": demand.mem_gb,
            "execution": {"backend": "flux", "profile": profile}}),
        )
    }

    pub(super) fn bind_attempt(
        &self,
        request: &mut Value,
        attempt: &Path,
        delivery: &Delivery,
        records: &Path,
    ) -> Result<()> {
        if request["inputs"].get("tests").is_none() {
            return Ok(());
        }
        let flux = self.flux()?;
        let options = crate::production::flux_options(flux);
        let profile = Profile {
            pass_flux_uri: self.1.pass_flux_uri(&delivery.repository),
            activation: flux.activation.clone(),
            schema: "tl3.worker_execution.v1".into(),
            attempt: fs::canonicalize(attempt)?,
            checkout: fs::canonicalize(
                request["worktree"]
                    .as_str()
                    .ok_or(Error::Invalid("worker checkout missing"))?,
            )?,
            delivery: delivery.clone(),
            options: options.bootstrap.config,
            job_environment: options.journal_root.join("job-environment.json"),
            staging_root: flux.staging_root.clone(),
            runtime_bundles: flux.runtime_bundles.clone(),
            container_mode: flux.container_mode,
            waits_root: records
                .parent()
                .ok_or(Error::Invalid("worker records need parent"))?
                .join("worker-waits"),
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
    #[serde(default)]
    pub pass_flux_uri: bool,
    #[serde(default)]
    pub activation: Option<tl3_config::FluxActivation>,
    pub schema: String,
    pub attempt: PathBuf,
    pub checkout: PathBuf,
    pub delivery: Delivery,
    pub options: PathBuf,
    pub job_environment: PathBuf,
    pub staging_root: PathBuf,
    pub runtime_bundles: Vec<PathBuf>,
    pub container_mode: tl3_config::ContainerMode,
    pub waits_root: PathBuf,
}

fn private_create(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    File::open(
        path.parent()
            .ok_or(Error::Invalid("worker profile needs parent"))?,
    )?
    .sync_all()?;
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
        if !backend::hex(&self.call, 64) {
            return Err(Error::Invalid(
                "worker call needs its retained fresh identity",
            ));
        }
        Ok(format!(
            "worker:{}",
            durable::digest(&serde_json::to_vec(&(
                &self.profile,
                &self.input,
                &self.served_tree,
                &self.call
            ))?)
        ))
    }

    pub fn validate(
        &self,
        key: &str,
        checkout: &Path,
        options: &backend::Options,
    ) -> Result<Profile> {
        let profile: Profile =
            serde_json::from_slice(&durable::read(&self.profile).map_err(backend_error)?)?;
        if self.key()? != key
            || self.owner.owner != "worker-tools"
            || self.owner.correlation != key.replace("worker:", "worker-")
            || self.intent.owner != self.owner.owner
            || self.intent.correlation != self.owner.correlation
            || profile.schema != "tl3.worker_execution.v1"
            || profile.checkout != fs::canonicalize(checkout)?
            || self.profile != profile.attempt.join("execution.json")
            || profile.options != options.bootstrap.config
            || profile.job_environment != options.journal_root.join("job-environment.json")
            || !backend::hex(&self.served_tree, 40)
        {
            return Err(Error::Invalid(
                "worker wait does not match its retained attempt and selected backend",
            ));
        }
        let request = read(&profile.attempt.join("request.json"))?
            .ok_or(Error::Invalid("worker wait attempt request missing"))?;
        if request["inputs"]["tests"]["execution"]["profile"] != json!(self.profile)
            || request["worktree"]
                .as_str()
                .map(Path::new)
                .map(fs::canonicalize)
                .transpose()?
                .as_ref()
                != Some(&profile.checkout)
        {
            return Err(Error::Invalid(
                "worker wait belongs to another attempt checkout",
            ));
        }
        if let Some(binding) = &self.binding {
            binding
                .id
                .validate()
                .map_err(|error| backend_error(error.into()))?;
            if binding.intent != self.intent
                || binding.id.uri != self.intent.uri
                || binding.id.instance != self.intent.instance
            {
                return Err(Error::Invalid("worker wait native binding changed"));
            }
        }
        let retained = durable::Journal::new(options.journal_root.clone())
            .map_err(backend_error)?
            .load(&self.owner)
            .map_err(backend_error)?;
        if let Some(binding) = &retained.binding {
            binding
                .id
                .validate()
                .map_err(|error| backend_error(error.into()))?;
            if binding.intent != self.intent
                || binding.id.uri != self.intent.uri
                || binding.id.instance != self.intent.instance
            {
                return Err(Error::Invalid("durable worker native binding changed"));
            }
        }
        if retained.intent != self.intent
            || retained.request.tree != self.input
            || !matches!(
                retained.request.suite,
                backend::Suite::WorkerTests { .. } | backend::Suite::Generator { .. }
            )
            || retained.intent.request_digest
                != durable::digest(&serde_json::to_vec(&retained.request)?)
            || self
                .binding
                .as_ref()
                .is_some_and(|binding| retained.binding.as_ref() != Some(binding))
        {
            return Err(Error::Invalid(
                "worker wait differs from durable backend request",
            ));
        }
        Ok(profile)
    }
}

fn backend_error(error: backend::BackendError) -> Error {
    Error::Adapter(Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PB-REMOVAL: without a selected Flux executor an attempt that carries worker tests is
    /// refused before any execution profile is written; one without tests needs no executor.
    #[test]
    fn prismabuild_selection_refuses_to_bind_a_tests_attempt() {
        let repo = crate::git_tests::Repo::new();
        let root = repo.root.path();
        let attempt = root.join("attempt-00000000");
        fs::create_dir(&attempt).unwrap();
        let retired = Selected(None, Default::default());
        let mut tests = json!({"worktree": repo.path, "inputs": {"tests": {
            "image": format!("sha256:{}", "a".repeat(64)), "cpus": 1, "mem_gb": 2,
            "execution": {"backend": "flux", "profile": attempt.join("execution.json")}}}});
        let error = retired
            .bind_attempt(
                &mut tests,
                &attempt,
                &repo.delivery(),
                &root.join("records/workers"),
            )
            .unwrap_err();
        assert!(
            matches!(
                &error,
                Error::Invalid(message) if message.contains("execution_backend = 'flux'")
            ),
            "{error:?}"
        );
        assert!(!attempt.join("execution.json").exists());
        let mut plain = json!({"worktree": repo.path, "inputs": {}});
        retired
            .bind_attempt(
                &mut plain,
                &attempt,
                &repo.delivery(),
                &root.join("records/workers"),
            )
            .unwrap();
        assert!(matches!(retired.image(), Err(Error::Invalid(_))));
    }

    #[test]
    fn worker_wait_rejects_other_attempt_checkout_generation_and_unrelated_suite() {
        let repo = crate::git_tests::Repo::new();
        let root = repo.root.path();
        let attempt = root.join("attempt-00000000");
        fs::create_dir(&attempt).unwrap();
        let flux = tl3_config::Flux {
            activation: None,
            instance_file: backend::fixture_instance(root),
            executable: "/fixture/flux".into(),
            backend_cli: "/fixture/tl3-run-backend".into(),
            container_mode: tl3_config::ContainerMode::OwnScope,
            runtime_bundles: vec![],
            environment: BTreeMap::new(),
            staging_root: root.join("staging"),
            output_root: root.join("output"),
            container_image: format!("sha256:{}", "a".repeat(64)),
        };
        let selected = Selected(Some(Box::new(flux.clone())), Default::default());
        let mut worker = json!({"worktree": repo.path, "inputs": {"tests": selected.inputs(&flux.container_image,
            tl3_config::TestDemand::default(), &attempt.join("execution.json")).unwrap()}});
        selected
            .bind_attempt(
                &mut worker,
                &attempt,
                &repo.delivery(),
                &root.join("records/workers"),
            )
            .unwrap();
        write(&attempt.join("request.json"), &worker).unwrap();
        let options = crate::production::flux_options(&flux);
        let tree = backend::TreeIdentity {
            head: "a".repeat(40),
            tree: "b".repeat(40),
            input_digest: "c".repeat(64),
            root: flux.staging_root.join("immutable-call"),
            manifest: flux.staging_root.join("input-manifest.json"),
        };
        let blob = backend::BlobRef {
            path: root.join("blob"),
            sha256: "d".repeat(64),
            bytes: 1,
        };
        let request = backend::Request {
            pass_flux_uri: false,
            repo: "owner/sandbox".into(),
            tree: tree.clone(),
            suite: backend::Suite::WorkerTests {
                input_digest: "e".repeat(64),
            },
            reservation: backend::Reservation {
                cpus: 2.try_into().unwrap(),
                gpus: 0,
                memory_bytes: 4.try_into().unwrap(),
                memory_guarantee: backend::MemoryGuarantee::DeclaredOnly,
                parallelism: 2.try_into().unwrap(),
                native_threads: 1.try_into().unwrap(),
            },
            runner: backend::RunnerIdentity {
                executable: root.join("collector"),
                executable_digest: "f".repeat(64),
                arguments: vec![],
                manifest: root.join("runner.json"),
                manifest_digest: "f".repeat(64),
            },
            runtime: backend::RuntimeIdentity {
                manifest: root.join("runtime.json"),
                manifest_digest: "f".repeat(64),
                runtime: backend::ExecutionRuntime::Native {
                    interpreter: "/fixture/python".into(),
                },
                architecture: "x86_64".into(),
                expected_image: None,
                preflight: blob,
                host_dependency: None,
                image: None,
                cleanup: None,
            },
            environment: BTreeMap::new(),
            scratch_root: root.join("scratch"),
            no_progress_seconds: None,
            walltime_seconds: 60.try_into().unwrap(),
        };
        let mut wait = WaitBinding {
            profile: attempt.join("execution.json"),
            owner: backend::Owner {
                owner: "worker-tools".into(),
                correlation: String::new(),
            },
            intent: backend::SubmissionIntent {
                backend: backend::BackendTag::Flux,
                instance: "runtime".into(),
                uri: options.resolved_uri().unwrap(),
                owner: "worker-tools".into(),
                correlation: String::new(),
                request_digest: durable::digest(&serde_json::to_vec(&request).unwrap()),
                output_dir: root.join("output"),
            },
            binding: None,
            input: tree,
            served_tree: "1".repeat(40),
            call: "2".repeat(64),
        };
        let key = wait.key().unwrap();
        wait.owner.correlation = key.replace("worker:", "worker-");
        wait.intent.correlation = wait.owner.correlation.clone();
        let journal = durable::Journal::new(options.journal_root.clone()).unwrap();
        let mut record = durable::Record {
            request,
            intent: wait.intent.clone(),
            dispatched: true,
            binding: None,
            cancel_requested: false,
            events: vec![],
            terminal: None,
            runtime_ownerships: vec![],
            container_launches: BTreeMap::new(),
            cleanup_evidence: BTreeMap::new(),
            jobspec: None,
            allocation_grant: None,
            instance_observation: None,
        };
        journal
            .transaction(&wait.owner, |dir, _| durable::save(dir, &record))
            .unwrap();
        assert!(wait.validate(&key, &repo.path, &options).is_ok());
        let changed = options.clone();
        fs::write(&changed.instance_file, "local:///replacement\n").unwrap();
        assert!(wait.validate(&key, &repo.path, &changed).is_ok());
        assert!(wait.validate(&key, root, &options).is_err());
        assert!(wait.validate("fABC", &repo.path, &options).is_err());
        let mut repeated = wait.clone();
        repeated.call = "3".repeat(64);
        assert_ne!(
            wait.key().unwrap(),
            repeated.key().unwrap(),
            "intentional repeats need separate references"
        );
        record.binding = Some(backend::RunBinding {
            intent: wait.intent.clone(),
            id: backend::BackendId {
                backend: backend::BackendTag::Flux,
                uri: options.resolved_uri().unwrap(),
                instance: "another-generation".into(),
                job: "fABC".into(),
            },
        });
        journal
            .transaction(&wait.owner, |dir, _| durable::save(dir, &record))
            .unwrap();
        assert!(wait.validate(&key, &repo.path, &options).is_err());
        record.binding = None;
        record.request.suite = backend::Suite::Full {
            membership_digest: "4".repeat(64),
        };
        journal
            .transaction(&wait.owner, |dir, _| durable::save(dir, &record))
            .unwrap();
        assert!(wait.validate(&key, &repo.path, &options).is_err());
        worker["inputs"]["tests"]["execution"]["profile"] =
            json!(root.join("another-attempt/execution.json"));
        write(&attempt.join("request.json"), &worker).unwrap();
        assert!(wait.validate(&key, &repo.path, &options).is_err());
    }
}
