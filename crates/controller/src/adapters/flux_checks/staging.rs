//! Immutable controller input construction, shared with the existing trusted check payload.
use super::*;
use crate::adapters::prismabuild::checks;
use std::os::unix::fs::PermissionsExt;
use tl3_worktrees::Worktrees;

/// Recorded in canary packets staged without an activation record (FX-REQUAL-43 OQ-1).
#[cfg(feature = "activation-qualification")]
pub const CANARY_QUALIFICATION_MARKER: &str = "tl3.fx_canary_qualification_staging.v1";

/// What binds staged requests to a qualified deployment.
enum Gate {
    Activated(Box<super::activation::RuntimeActivation>),
    /// Canary packet staging before a pin exists: no record, structural preflight only.
    /// Only the fx-stage-packet example enables this; deployed binaries never compile it.
    #[cfg(feature = "activation-qualification")]
    CanaryQualification,
}

impl Gate {
    fn validate_options(&self, options: &backend::Options) -> Result<()> {
        match self {
            Self::Activated(activation) => activation.validate_options(options),
            #[cfg(feature = "activation-qualification")]
            Self::CanaryQualification => Ok(()),
        }
    }

    fn validate_prepared(
        &self,
        prepared: &backend::PreparedRuntime,
        reservation: &backend::Reservation,
    ) -> Result<()> {
        match self {
            Self::Activated(activation) => activation.validate_prepared(prepared, reservation),
            #[cfg(feature = "activation-qualification")]
            Self::CanaryQualification => {
                crate::adapters::run_backend::flux::validate_prepared_runtime(prepared)
                    .map_err(|_| Error::Invalid("flux.activation.preflight"))
            }
        }
    }
}

/// Production staging settings. Job environment is supplied by the one production mapper.
pub struct Stager {
    config: tl3_config::Flux,
    gate: Gate,
    images: crate::adapters::images::Images,
    protected: crate::scope::ProtectedPaths,
    global_protected: Vec<String>,
    no_progress: std::num::NonZeroU32,
}

impl Stager {
    pub fn new(config: &tl3_config::Config) -> Result<Self> {
        Self::gated(config, |flux| {
            Ok(Gate::Activated(Box::new(
                super::activation::RuntimeActivation::load(flux)?,
            )))
        })
    }

    /// Stage a canary packet before the activation pin exists. Refuses a configured record,
    /// which must use the gated `new`. Example-only: deployed binaries never enable the feature.
    #[cfg(feature = "activation-qualification")]
    pub fn for_canary_qualification(config: &tl3_config::Config) -> Result<Self> {
        Self::gated(config, |flux| {
            if flux.activation.is_some() {
                return Err(Error::Invalid("flux.activation.canary_qualification"));
            }
            if !tl3_config::immutable_image(&flux.container_image) {
                return Err(Error::Invalid("flux.activation.image"));
            }
            Ok(Gate::CanaryQualification)
        })
    }

    /// The packet marker for a canary-qualification Stager; `None` for every activated one.
    #[cfg(feature = "activation-qualification")]
    pub fn canary_qualification_marker(&self) -> Option<&'static str> {
        match self.gate {
            Gate::Activated(_) => None,
            Gate::CanaryQualification => Some(CANARY_QUALIFICATION_MARKER),
        }
    }

    fn gated(
        config: &tl3_config::Config,
        gate: impl FnOnce(&tl3_config::Flux) -> Result<Gate>,
    ) -> Result<Self> {
        let tl3_config::SelectedExecution::Flux(flux) = config.selected_execution()? else {
            return Err(Error::Invalid(
                "Flux preparation requires explicit Flux selection",
            ));
        };
        Ok(Self {
            config: flux.clone(),
            gate: gate(flux)?,
            images: crate::adapters::images::Images::new(&config.github.repos),
            protected: crate::scope::ProtectedPaths::new(&config.github.repos),
            global_protected: config.planning.protected_paths.clone(),
            no_progress: config
                .allowances
                .prismabuild
                .no_progress_secs
                .try_into()
                .map_err(|_| Error::Invalid("positive check no-progress allowance required"))?,
        })
    }

    fn reservation(&self, delivery: &Delivery) -> Result<backend::Reservation> {
        let demand = self
            .images
            .test_demand(&delivery.repository)
            .unwrap_or_default();
        let parallel = delivery
            .checks
            .as_ref()
            .into_iter()
            .flat_map(|run| &run.plan)
            .filter_map(|check| Some(check.declared_symptom.as_ref()?.parallel.get()))
            .max()
            .unwrap_or(1)
            .clamp(1, 4) as u32;
        super::super::workers::execution::reservation(demand.cpus, demand.mem_gb, parallel)

    }

    /// Build immutable input and shared runtime identities without launching native work.
    pub fn prepare_request(&self, delivery: &Delivery) -> Result<PreparedCheck> {
        self.gate
            .validate_options(&crate::production::flux_options(&self.config))?;
        let reservation = self.reservation(delivery)?;
        let image = self
            .images
            .for_repository(&delivery.repository, &self.config.container_image);
        let root = input_root(&self.config.staging_root, delivery)?;
        // The collector supports at most one day. Existing no-progress allowance still bounds silence.
        let walltime = std::num::NonZeroU32::new(86400).expect("positive collector bound");
        let arguments = vec![
            "--timeout".into(),
            walltime.to_string(),
            "--".into(),
            "/usr/bin/python3.14".into(),
            "-I".into(),
            "-B".into(),
            root.join("tree/action.py")
                .to_str()
                .ok_or(Error::Invalid("UTF-8 input path required"))?
                .into(),
        ];
        let prepared = select_runtime(
            &self.config.runtime_bundles,
            image,
            self.config.container_mode,
            &reservation,
            arguments,
        )?;
        self.gate.validate_prepared(&prepared, &reservation)?;
        let backend::ExecutionRuntime::RootlessPodman { trusted_root, .. } =
            &prepared.runtime.runtime
        else {
            return Err(Error::Invalid(
                "daytime checks require the prepared Podman runtime",
            ));
        };
        let protected = self
            .protected
            .for_repository(&delivery.repository, &self.global_protected)?;
        let tests = self.images.additional_tests(&delivery.repository);
        fs::create_dir_all(&self.config.staging_root)?;
        let input = stage_input(delivery, &root, trusted_root, image, &protected, &tests)?;
        let head = delivery
            .head_sha
            .as_ref()
            .ok_or(Error::Invalid("missing served head"))?;
        let request = backend::Request {
            pass_flux_uri: self.images.pass_flux_uri(&delivery.repository),
            repo: format!("{}/{}", delivery.repository.owner, delivery.repository.name),
            tree: input.tree,
            suite: backend::Suite::DaytimeChecks {
                plan_digest: durable::digest(&serde_json::to_vec(&input.run.plan)?),
                base: input.run.base_sha.as_str().into(),
                head: head.as_str().into(),
                reviewed: input
                    .run
                    .reviewed_sha
                    .as_ref()
                    .map(|sha| sha.as_str().into()),
            },
            reservation,
            runner: prepared.runner,
            runtime: prepared.runtime,
            environment: crate::production::flux_job_environment(),
            scratch_root: prepared.scratch_root,
            no_progress_seconds: Some(self.no_progress),
            walltime_seconds: walltime,
        };
        Ok(PreparedCheck {
            request,
            run: input.run,
            changed_paths: input.changed_paths,
            selected_tests: input.selected_tests,
            always_tests: input.always_tests,
        })
    }
}

impl Preparation for Stager {
    fn prepare(&mut self, delivery: &Delivery) -> Result<PreparedCheck> {
        self.prepare_request(delivery)
    }
    fn capacity(&self, delivery: &Delivery) -> Result<(backend::Reservation, backend::Placement)> {
        self.gate
            .validate_options(&crate::production::flux_options(&self.config))?;
        let reservation = self.reservation(delivery)?;
        let image = self
            .images
            .for_repository(&delivery.repository, &self.config.container_image);
        let prepared = select_runtime(
            &self.config.runtime_bundles,
            image,
            self.config.container_mode,
            &reservation,
            vec![],
        )?;
        self.gate.validate_prepared(&prepared, &reservation)?;
        Ok((reservation, prepared.runtime.placement()))
    }
}

fn input_root(parent: &Path, delivery: &Delivery) -> Result<PathBuf> {
    use std::io::Read;
    let mut nonce = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut nonce)?;
    // Each attempt keeps its own immutable publication. Failed or unreferenced
    // attempts are retained for ownership-checked retention, never overwritten.
    Ok(parent.join(format!(
        "{}-{}",
        correlation(delivery),
        durable::digest(&nonce)
    )))
}

pub struct PreparedInput {
    pub tree: backend::TreeIdentity,
    pub run: CheckRun,
    pub changed_paths: Vec<String>,
    pub selected_tests: Option<u32>,
    pub always_tests: Vec<String>,
}

/// `root` is a fresh private shared destination, outside the served checkout.
/// The trusted harness comes from the provisioned immutable release, never the served tree.
pub(in crate::adapters) fn stage_input(
    delivery: &Delivery,
    root: &Path,
    trusted_harness: &Path,
    image: &str,
    protected_paths: &[String],
    tests: &crate::adapters::images::AdditionalTests,
) -> Result<PreparedInput> {
    if !backend::absolute(root) {
        return Err(Error::Invalid(
            "Flux input requires a new absolute destination",
        ));
    }
    // The shared stage function supports PB replacement. Flux publication never replaces
    // a retained input, including a partial publication or a dangling symlink.
    let run = checks::descended_base(delivery)?;
    let served = run.repository.canonicalize()?;
    let parent = root
        .parent()
        .ok_or(Error::Invalid("input requires a parent directory"))?;
    if parent.canonicalize()?.starts_with(&served)
        || trusted_harness.canonicalize()?.starts_with(&served)
    {
        return Err(Error::Invalid(
            "trusted Flux inputs must stay outside the served checkout",
        ));
    }
    for ancestor in root.ancestors() {
        if ancestor.is_symlink() {
            return Err(Error::Invalid("Flux input ancestors cannot be symlinks"));
        }
    }
    fs::create_dir(root)?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    let tree_root = root.join("tree");
    let mut staged = delivery.clone();
    staged.checks = Some(run.clone());
    let changed_paths = checks::stage(
        &tree_root,
        trusted_harness,
        &staged,
        image,
        protected_paths,
        tests,
    )?;
    let repository = Worktrees::open(&tree_root, &tree_root)?;
    let head = repository.head()?;
    let tree = repository.tree(&head)?;
    let mut files = Vec::new();
    inventory(&tree_root, &tree_root, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest = backend::InputManifest {
        schema: "tl3.input_manifest.v1".into(),
        head: head.as_str().into(),
        tree: tree.clone(),
        files,
    };
    let bytes = serde_json::to_vec(&manifest)?;
    if bytes.len() > durable::MAX_BYTES {
        return Err(Error::Invalid("Flux input manifest exceeds byte bound"));
    }
    let manifest_path = root.join("input-manifest.json");
    durable::atomic(&manifest_path, &bytes).map_err(adapter_error)?;
    fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o400))?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o500))?;
    File::open(root)?.sync_all()?;
    if let Some(parent) = root.parent() {
        File::open(parent)?.sync_all()?;
    }
    let mut seen = std::collections::BTreeSet::new();
    Ok(PreparedInput {
        tree: backend::TreeIdentity {
            head: head.as_str().into(),
            tree,
            input_digest: durable::digest(&bytes),
            root: tree_root,
            manifest: manifest_path,
        },
        run,
        changed_paths,
        selected_tests: tests.selected,
        always_tests: tests
            .always
            .iter()
            .filter(|test| seen.insert((*test).clone()))
            .cloned()
            .collect(),
    })
}

fn inventory(root: &Path, directory: &Path, files: &mut Vec<backend::InputFile>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if directory == root && entry.file_name() == ".git" {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            inventory(root, &path, files)?;
        } else if kind.is_file() {
            if files.len() >= 100_000 {
                return Err(Error::Invalid("Flux input inventory exceeds file bound"));
            }
            let bytes = durable::read(&path).map_err(adapter_error)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o400))?;
            File::open(&path)?.sync_all()?;
            files.push(backend::InputFile {
                path: path
                    .strip_prefix(root)
                    .map_err(|_| Error::Invalid("input outside staged root"))?
                    .to_owned(),
                sha256: durable::digest(&bytes),
            });
        } else {
            return Err(Error::Invalid(
                "Flux inputs must contain regular files, without symlinks",
            ));
        }
    }
    fs::set_permissions(directory, fs::Permissions::from_mode(0o500))?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

/// Select exactly one already-prepared image/mode identity. The shared producer owns
/// inventory resolution, original map provenance and all bundle byte/mode checks.
pub fn select_runtime(
    bundles: &[PathBuf],
    image: &str,
    mode: tl3_config::ContainerMode,
    reservation: &backend::Reservation,
    arguments: Vec<String>,
) -> Result<backend::PreparedRuntime> {
    if !tl3_config::immutable_image(image) {
        return Err(Error::Invalid("flux.activation.image"));
    }
    let mut selected = None;
    for bundle in bundles {
        let prepared = backend::prepared::prepare_runtime(bundle, reservation, arguments.clone())
            .map_err(adapter_error)?;
        let Some(identity) = &prepared.runtime.image else {
            return Err(Error::Invalid(
                "prepared runtime has no immutable image identity",
            ));
        };
        if image != identity.reference && image != identity.config_digest {
            continue;
        }
        if !matches!(&prepared.runtime.runtime, backend::ExecutionRuntime::RootlessPodman { mode: prepared_mode, .. } if *prepared_mode == mode)
        {
            continue;
        }
        if selected.replace(prepared).is_some() {
            return Err(Error::Invalid(
                "configured image and mode match multiple immutable runtime bundles",
            ));
        }
    }
    selected.ok_or(Error::Invalid(
        "configured immutable image and selected mode have no prepared runtime bundle",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Read-only publication is the production contract; fixture teardown restores owner write access.
    fn writable(directory: &Path) {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                writable(&entry.path());
            }
        }
    }

    /// FX / #750: transport identity covers committed served input and the external trusted harness.
    #[test]
    fn staged_transport_preserves_served_identity_and_manifests_every_payload_file() {
        let root = tempfile::tempdir().unwrap();
        let fixture = crate::git_tests::Repo::new();
        let d = fixture.delivery();
        fs::write(fixture.path.join("README.md"), "dirty maker data").unwrap();
        let harness = root.path().join("trusted-release");
        fs::create_dir(&harness).unwrap();
        fs::write(harness.join("runner.py"), "# trusted fixture\n").unwrap();
        fs::write(harness.join("__init__.py"), "").unwrap();
        let destination = root.path().join("input");
        let result = stage_input(
            &d,
            &destination,
            &harness,
            "immutable-image",
            &["protected/**".into()],
            &crate::adapters::images::AdditionalTests {
                selected: Some(3),
                always: vec!["tests/test_always.py".into()],
                generated: vec!["generated/**".into()],
            },
        );
        if result.is_err() {
            writable(root.path());
        }
        let staged = result.unwrap();
        let manifest: backend::InputManifest =
            serde_json::from_slice(&fs::read(&staged.tree.manifest).unwrap()).unwrap();
        assert_eq!(manifest.head, staged.tree.head);
        assert_eq!(manifest.tree, staged.tree.tree);
        assert_ne!(staged.tree.head, d.head_sha.as_ref().unwrap().as_str());
        assert_eq!(
            manifest
                .files
                .iter()
                .map(|file| file.path.to_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "action.py",
                "request.json",
                "served.bundle",
                "trusted/tl3_harness/__init__.py",
                "trusted/tl3_harness/runner.py"
            ]
        );
        for file in &manifest.files {
            let path = staged.tree.root.join(&file.path);
            assert!(fs::metadata(&path).unwrap().permissions().readonly());
            assert_eq!(durable::digest(&fs::read(path).unwrap()), file.sha256);
        }
        let request: serde_json::Value =
            serde_json::from_slice(&fs::read(staged.tree.root.join("request.json")).unwrap())
                .unwrap();
        assert_eq!(request["head"], fixture.head);
        assert_eq!(request["base"], fixture.base);
        assert_eq!(
            request["protected_paths"],
            serde_json::json!(["protected/**"])
        );
        assert_eq!(request["footprint"], serde_json::json!(d.paths));
        assert_eq!(
            request["generated_paths"],
            serde_json::json!(["generated/**"])
        );
        assert_eq!(request["selected_tests"], 3);
        assert_eq!(
            request["always_tests"],
            serde_json::json!(["tests/test_always.py"])
        );
        assert_eq!(staged.changed_paths, ["README.md"]);
        assert!(
            stage_input(
                &d,
                &destination,
                &harness,
                "changed-image",
                &[],
                &crate::adapters::images::AdditionalTests {
                    selected: None,
                    always: vec![],
                    generated: vec![],
                },
            )
            .is_err()
        );
        assert_eq!(
            durable::digest(&fs::read(&staged.tree.manifest).unwrap()),
            staged.tree.input_digest
        );
        writable(root.path());
    }
    #[test]
    fn staging_retries_preserve_partial_and_published_inputs() {
        let root = tempfile::tempdir().unwrap();
        let fixture = crate::git_tests::Repo::new();
        let delivery = fixture.delivery();
        let harness = root.path().join("trusted");
        fs::create_dir(&harness).unwrap();
        fs::write(harness.join("runner.py"), "# trusted\n").unwrap();
        let partial = input_root(root.path(), &delivery).unwrap();
        fs::create_dir(&partial).unwrap();
        fs::write(partial.join("retained"), b"partial before crash").unwrap();
        let tests = crate::adapters::images::AdditionalTests {
            selected: None,
            always: vec![],
            generated: vec![],
        };
        let next = input_root(root.path(), &delivery).unwrap();
        let staged = stage_input(&delivery, &next, &harness, "image", &[], &tests)
            .expect("partial staging must not prevent a fresh attempt");
        let retry = input_root(root.path(), &delivery).unwrap();
        let again = stage_input(&delivery, &retry, &harness, "image", &[], &tests)
            .expect("publication before job save must permit a fresh attempt");
        assert_ne!(staged.tree.root, again.tree.root);
        assert_eq!(
            fs::read(partial.join("retained")).unwrap(),
            b"partial before crash"
        );
        for input in [staged, again] {
            assert_eq!(
                durable::digest(&fs::read(&input.tree.manifest).unwrap()),
                input.tree.input_digest
            );
            assert_eq!(
                input.run.base_sha,
                delivery.checks.as_ref().unwrap().base_sha
            );
        }
        writable(root.path());
    }
}
