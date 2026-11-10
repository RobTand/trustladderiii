use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[path = "../../../../tools/generate_fx_qualification.rs"]
mod qualification_generator;

fn assert_refusal<T>(result: Result<T>, field: &str) {
    assert_eq!(
        result
            .err()
            .expect("expected activation refusal")
            .to_string(),
        field
    );
}

pub(crate) struct Fixture {
    pub root: tempfile::TempDir,
    pub flux: tl3_config::Flux,
    pub record: DeploymentRecord,
    pub input: backend::PrepareBundleInput,
    bundles: Vec<PathBuf>,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("fx-activation-test-")
            .tempdir_in("/mnt/shared")
            .unwrap();
        let source = root.path().join("source");
        let controller = std::env::var_os("TL3_FX_QUALIFICATION_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let checkout = controller.join("../..");
        let current = controller.join("qualification/current.json").is_file();
        let fixtures = controller.join(if current {
            "qualification/fx-current/fixtures"
        } else {
            "qualification/fx-exact42b/fixtures"
        });
        let qualification: Qualification = serde_json::from_slice(PIN).unwrap();
        for file in &qualification.files {
            let path = source.join(&file.path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let name = file.path.file_name().unwrap();
            let original = if current {
                fixtures.join(&file.path)
            } else if fixtures.join(name).is_file() {
                fixtures.join(name)
            } else {
                checkout.join("harness/src/tl3_harness").join(name)
            };
            let raw = fs::read(original).unwrap();
            assert_eq!(
                durable::digest(&raw),
                file.sha256,
                "qualified source {} changed",
                file.path.display()
            );
            fs::write(path, raw).unwrap();
        }
        let image = format!("fixture.invalid/activation@sha256:{}", "a".repeat(64));
        let config_digest = format!("sha256:{}", "b".repeat(64));
        let inventory = root.path().join("image-map.json");
        fs::write(&inventory, serde_json::to_vec(&serde_json::json!({"profiles":[{
            "profile":"activation-fixture", "registry_ref":image, "manifest_digest":format!("sha256:{}", "a".repeat(64)),
            "config_digest":config_digest,"platform":"linux/amd64"
        }]})).unwrap()).unwrap();
        let inspect = root.path().join("image-inspect.json");
        fs::write(
            &inspect,
            serde_json::to_vec(
                &serde_json::json!({"Id":config_digest,"Os":"linux","Architecture":"amd64"}),
            )
            .unwrap(),
        )
        .unwrap();
        let cli = root.path().join("synthetic-cli");
        fs::write(&cli, b"synthetic identity only; never executed").unwrap();
        let input = backend::PrepareBundleInput {
            source_root: source,
            code_inventory: "collector-manifest.json".into(),
            collector: "host-collector".into(),
            cleanup: "host-cleanup".into(),
            trusted_root: "lib/tl3_harness".into(),
            producer_commit: "f".repeat(40),
            container_interpreter: "/opt/tl3/bin/python".into(),
            cleanup_arguments: vec![],
            image_inventory: inventory,
            image_inspect: inspect,
            profile: "activation-fixture".into(),
            mode: ContainerMode::InJob,
            prerequisites: backend::ModePrerequisites {
                no_cgroups_supported: true,
                init_available: true,
                user_scope_supported: false,
            },
        };
        let bundle = backend::prepared::prepare_bundle(&input).unwrap();
        let prepared =
            backend::prepared::prepare_runtime(&bundle.path, &registration_reservation(), vec![])
                .unwrap();
        let flux = tl3_config::Flux {
            activation: Some(FluxActivation {
                record: root.path().join("activation.json"),
                sha256: String::new(),
            }),
            instance_file: backend::fixture_instance(root.path()),
            executable: cli.clone(),
            backend_cli: cli,
            container_mode: ContainerMode::InJob,
            runtime_bundles: vec![bundle.path.clone()],
            environment: BTreeMap::from([
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("HOME".into(), "/home/rob".into()),
                ("XDG_RUNTIME_DIR".into(), "/run/user/1000".into()),
            ]),
            staging_root: root.path().join("staging"),
            output_root: root.path().join("outputs"),
            container_image: image,
        };
        let options = crate::production::flux_options(&flux);
        let record = DeploymentRecord {
            schema: "tl3.fx_activation.v2".into(),
            qualification_sha256: qualification_sha256(),
            producer_commit: input.producer_commit.clone(),
            backend_options_sha256: options_identity(&options).unwrap(),
            backend_cli_sha256: executable_sha256(&options.bootstrap.cli).unwrap(),
            executable_sha256: executable_sha256(&options.executable).unwrap(),
            staging_root_sha256: identity(&flux.staging_root).unwrap(),
            container_mode: flux.container_mode,
            acceptance_scope: AcceptanceScope::PlannedFreshInit,
            profiles: vec![ProfileBinding {
                manifest: bundle.path.clone(),
                manifest_sha256: bundle.sha256,
                image: prepared.runtime.image.unwrap(),
                preflight: prepared.runtime.preflight,
                required_native_success_runs: 1.try_into().unwrap(),
                required_cleanup: CleanupRequirement::Verified,
            }],
        };
        let mut fixture = Self {
            root,
            flux,
            record,
            input,
            bundles: vec![bundle.path],
        };
        fixture.seal_record();
        fixture
    }

    pub(crate) fn seal_record(&mut self) {
        let raw = serde_json::to_vec(&self.record).unwrap();
        let pin = self.flux.activation.as_mut().unwrap();
        fs::write(&pin.record, &raw).unwrap();
        pin.sha256 = durable::digest(&raw);
    }

    fn replace_bundle(&mut self) {
        let bundle = backend::prepared::prepare_bundle(&self.input).unwrap();
        self.bundles.push(bundle.path.clone());
        let prepared =
            backend::prepared::prepare_runtime(&bundle.path, &registration_reservation(), vec![])
                .unwrap();
        self.flux.runtime_bundles = vec![bundle.path.clone()];
        self.record.profiles[0].manifest = bundle.path;
        self.record.profiles[0].manifest_sha256 = bundle.sha256;
        self.record.profiles[0].preflight = prepared.runtime.preflight;
        self.record.profiles[0].image = prepared.runtime.image.unwrap();
        self.seal_record();
    }

    fn update_code_inventory(&self, module: &str) {
        let code = self.input.source_root.join("collector-manifest.json");
        let mut inventory: backend::CodeInventory =
            serde_json::from_slice(&fs::read(&code).unwrap()).unwrap();
        let file = inventory
            .files
            .iter_mut()
            .find(|file| file.path == Path::new(module))
            .unwrap();
        let bytes = fs::read(self.input.source_root.join(module)).unwrap();
        file.bytes = bytes.len() as u64;
        file.sha256 = durable::digest(&bytes);
        fs::write(code, serde_json::to_vec(&inventory).unwrap()).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fn writable(path: &Path) {
            if path.is_dir() {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
                for child in fs::read_dir(path).unwrap() {
                    writable(&child.unwrap().path());
                }
            }
        }
        writable(self.root.path());
        for bundle in &self.bundles {
            let root = bundle.parent().unwrap();
            writable(root);
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn qualified_identity_and_exact_planned_deployment_pass_staging_boundary() {
    let fixture = Fixture::new();
    let activation = RuntimeActivation::load(&fixture.flux).unwrap();
    let reservation = registration_reservation();
    let prepared = crate::adapters::flux_checks::staging::select_runtime(
        &fixture.flux.runtime_bundles,
        &fixture.flux.container_image,
        fixture.flux.container_mode,
        &reservation,
        vec!["--fixture".into()],
    )
    .unwrap();
    activation
        .validate_prepared(&prepared, &reservation)
        .unwrap();
    assert_eq!(
        fixture.record.profiles[0]
            .required_native_success_runs
            .get(),
        1
    );
    let raw = fs::read(&fixture.flux.activation.as_ref().unwrap().record).unwrap();
    let text = String::from_utf8(raw).unwrap();
    assert!(text.contains("planned_fresh_init"));
    for private in [
        "local:///fixture",
        "\"instance_generation\"",
        "/usr/bin:/bin",
    ] {
        assert!(!text.contains(private));
    }
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn absent_wrong_hash_unknown_qualification_and_generation_refuse_with_fields_only() {
    let mut fixture = Fixture::new();
    let original = fixture.flux.activation.clone();
    fixture.flux.activation = None;
    assert_eq!(
        RuntimeActivation::load(&fixture.flux)
            .err()
            .unwrap()
            .to_string(),
        "flux.activation"
    );
    fixture.flux.activation = original;
    fixture.flux.activation.as_mut().unwrap().sha256 = "0".repeat(64);
    assert_refusal(
        RuntimeActivation::load(&fixture.flux),
        "flux.activation.sha256",
    );
    fixture.seal_record();
    fixture.record.qualification_sha256 = "0".repeat(64);
    fixture.seal_record();
    assert_refusal(
        RuntimeActivation::load(&fixture.flux),
        "flux.activation.qualification_sha256",
    );
    fixture.record.qualification_sha256 = qualification_sha256();
    fixture.seal_record();
    let activation = RuntimeActivation::load(&fixture.flux).unwrap();
    let mut options = crate::production::flux_options(&fixture.flux);
    fs::write(&options.instance_file, "local:///replacement\n").unwrap();
    options
        .environment
        .insert("FLUX_URI".into(), "local:///replacement".into());
    activation.validate_options(&options).unwrap();
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn planned_scope_requires_explicit_success_and_cleanup_and_never_completed_claims() {
    let mut fixture = Fixture::new();
    let original = serde_json::to_value(&fixture.record).unwrap();
    for field in [
        "acceptance_scope",
        "required_native_success_runs",
        "required_cleanup",
    ] {
        let mut value = original.clone();
        match field {
            "acceptance_scope" => value[field] = "completed-private-claim".into(),
            "required_native_success_runs" => value["profiles"][0][field] = 0.into(),
            _ => {
                value["profiles"][0].as_object_mut().unwrap().remove(field);
            }
        }
        let raw = serde_json::to_vec(&value).unwrap();
        let pin = fixture.flux.activation.as_mut().unwrap();
        fs::write(&pin.record, &raw).unwrap();
        pin.sha256 = durable::digest(&raw);
        let error = RuntimeActivation::load(&fixture.flux)
            .err()
            .unwrap()
            .to_string();
        assert_eq!(error, "flux.activation.record");
        assert!(!error.contains("private-claim"));
    }
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn bundle_set_hash_profile_preflight_options_and_cli_substitution_refuse() {
    let mut fixture = Fixture::new();
    fixture.record.profiles[0].manifest_sha256 = "0".repeat(64);
    fixture.seal_record();
    assert_refusal(
        RuntimeActivation::load(&fixture.flux),
        "flux.activation.manifest_sha256",
    );
    fixture.record.profiles[0].manifest_sha256 = fixture.flux.runtime_bundles[0]
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    fixture.record.profiles[0].preflight.sha256 = "0".repeat(64);
    fixture.seal_record();
    assert_refusal(
        RuntimeActivation::load(&fixture.flux),
        "flux.activation.profile",
    );
    fixture.record.profiles[0].preflight.sha256 =
        durable::digest(&fs::read(&fixture.record.profiles[0].preflight.path).unwrap());
    fixture.seal_record();
    let mut options = crate::production::flux_options(&fixture.flux);
    let activation = RuntimeActivation::load(&fixture.flux).unwrap();
    options
        .environment
        .insert("PATH".into(), "/private/options-value".into());
    let error = activation
        .validate_options(&options)
        .unwrap_err()
        .to_string();
    assert!(error.ends_with("flux.activation.backend_options_sha256"));
    assert!(!error.contains("/private/options-value"));
    fs::write(&fixture.flux.backend_cli, b"changed CLI").unwrap();
    assert_refusal(
        RuntimeActivation::load(&fixture.flux),
        "flux.activation.backend_cli_sha256",
    );
    fixture
        .flux
        .runtime_bundles
        .push(fixture.flux.runtime_bundles[0].clone());
    assert_refusal(
        RuntimeActivation::load(&fixture.flux),
        "flux.activation.profiles",
    );
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn sealed_but_unqualified_full_closure_and_fault_interpreter_refuse() {
    let mut fixture = Fixture::new();
    let helper = "lib/tl3_harness/runtime_host.py";
    fs::write(
        fixture.input.source_root.join(helper),
        b"changed helper, still sealed",
    )
    .unwrap();
    fixture.update_code_inventory(helper);
    fixture.replace_bundle();
    assert_refusal(
        RuntimeActivation::load(&fixture.flux),
        "flux.activation.code_inventory",
    );
    drop(fixture);
    let mut fixture = Fixture::new();
    fixture.input.container_interpreter = "/opt/tl3/bin/python-canary2-absent".into();
    fixture.replace_bundle();
    assert_refusal(
        RuntimeActivation::load(&fixture.flux),
        "flux.activation.metadata",
    );
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn missing_changed_cleanup_and_extra_unlisted_module_refuse() {
    for change in ["missing", "changed", "extra"] {
        let fixture = Fixture::new();
        let root = fixture.flux.runtime_bundles[0].parent().unwrap();
        let cleanup = root.join("host-cleanup");
        match change {
            "missing" => {
                fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
                fs::remove_file(cleanup).unwrap();
            }
            "changed" => {
                fs::set_permissions(&cleanup, fs::Permissions::from_mode(0o700)).unwrap();
                fs::write(cleanup, b"changed").unwrap();
            }
            _ => {
                fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
                fs::write(root.join("extra-helper.py"), b"extra").unwrap();
            }
        }
        assert_refusal(
            RuntimeActivation::load(&fixture.flux),
            "flux.activation.bundle",
        );
    }
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn tags_runtime_substitution_and_environment_override_cannot_grant_activation() {
    let fixture = Fixture::new();
    let activation = RuntimeActivation::load(&fixture.flux).unwrap();
    for image in [
        "fixture.invalid/activation:latest",
        "fixture.invalid/activation",
        "sha256:abc",
    ] {
        assert!(activation.validate_image(image).is_err());
    }
    let reservation = registration_reservation();
    let mut prepared =
        backend::prepared::prepare_runtime(&fixture.flux.runtime_bundles[0], &reservation, vec![])
            .unwrap();
    prepared.runtime.cleanup.as_mut().unwrap().executable_digest = "0".repeat(64);
    assert!(
        activation
            .validate_prepared(&prepared, &reservation)
            .is_err()
    );
    // The bypass attempt runs in a child, leaving this process's environment untouched.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "adapters::flux_checks::activation::tests::environment_override_child",
            "--ignored",
            "--nocapture",
        ])
        .env("TL3_FLUX_RUNTIME_ACTIVATED", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
#[ignore = "subprocess only: isolates environment override attempt"]
fn environment_override_child() {
    let mut fixture = Fixture::new();
    fixture.flux.activation = None;
    assert_refusal(RuntimeActivation::load(&fixture.flux), "flux.activation");
}

#[test]
fn qualification_regeneration_is_byte_identical() {
    let root = std::env::var_os("TL3_FX_QUALIFICATION_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    let output = qualification_generator::generate_checkout(&root);
    assert!(output.is_ok(), "{}", output.as_ref().unwrap_err());
    assert_eq!(output.unwrap(), PIN);
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn qualified_binding_passes_real_controller_capacity_and_startup_activation() {
    use crate::adapters::flux_checks::Preparation;
    let mut fixture = Fixture::new();
    // Match the production config contract, without writing any backend/runtime state there.
    fixture.flux.output_root = "/mnt/shared/flux-out/tl3".into();
    let options = crate::production::flux_options(&fixture.flux);
    fixture.record.backend_options_sha256 = options_identity(&options).unwrap();
    fixture.seal_record();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../../config/example.toml")).unwrap();
    config.flux = fixture.flux.clone();
    for repo in &mut config.github.repos {
        repo.container_image = None;
    }
    config.github.app_credentials_file = fixture.root.path().join("missing-synthetic-github.json");
    config.models.key_file = fixture.root.path().join("missing-synthetic-model-key");
    let repo = crate::git_tests::Repo::new();
    let stager = crate::adapters::flux_checks::staging::Stager::new(&config).unwrap();
    let (_, placement) = stager.capacity(&repo.delivery()).unwrap();
    assert_eq!(placement.host_dependency.unwrap().host, "dl380g10");
    let mut stager = stager;
    stager.prepare(&repo.delivery()).unwrap();
    let error = crate::production::register(&config)
        .err()
        .expect("synthetic credential absence must stop before store");
    assert!(matches!(error, Error::Config(_)), "{error}");
    fs::write(&fixture.flux.backend_cli, b"substituted after registration").unwrap();
    assert_eq!(
        stager.capacity(&repo.delivery()).unwrap_err().to_string(),
        "flux.activation.backend_cli_sha256"
    );
    assert!(stager.prepare(&repo.delivery()).is_err());
}

/// FX-REQUAL-43 OQ-1: an activated Stager never carries the canary marker, and the
/// canary gate refuses a configured record. Built only with the example-only feature.
#[cfg(feature = "activation-qualification")]
#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn canary_qualification_gate_never_marks_an_activated_stager_and_refuses_records() {
    use crate::adapters::flux_checks::staging::{CANARY_QUALIFICATION_MARKER, Stager};
    let mut fixture = Fixture::new();
    fixture.flux.output_root = "/mnt/shared/flux-out/tl3".into();
    let options = crate::production::flux_options(&fixture.flux);
    fixture.record.backend_options_sha256 = options_identity(&options).unwrap();
    fixture.seal_record();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../../config/example.toml")).unwrap();
    config.execution_backend = tl3_config::ExecutionBackend::Flux;
    config.flux = fixture.flux.clone();
    for repo in &mut config.github.repos {
        repo.container_image = None;
    }
    let activated = Stager::new(&config).unwrap();
    assert_eq!(activated.canary_qualification_marker(), None);
    assert_refusal(
        Stager::for_canary_qualification(&config),
        "flux.activation.canary_qualification",
    );
    config.flux.activation = None;
    let canary = Stager::for_canary_qualification(&config).unwrap();
    assert_eq!(
        canary.canary_qualification_marker(),
        Some(CANARY_QUALIFICATION_MARKER)
    );
    assert_refusal(Stager::new(&config), "flux.activation");
}

#[test]
fn x7_activation_survives_broker_identity_changes() {
    let root = tempfile::tempdir().unwrap();
    let cli = root.path().join("cli");
    fs::write(&cli, "unchanged tool").unwrap();
    let mut options = backend::Options {
        executable: cli.clone(),
        arguments: vec![],
        environment: BTreeMap::new(),
        instance_file: backend::fixture_instance(root.path()),
        journal_root: root.path().join("journal"),
        bootstrap: backend::HostBootstrap {
            cli,
            config: root.path().join("options.json"),
        },
    };
    let record = DeploymentRecord {
        schema: "tl3.fx_activation.v2".into(),
        qualification_sha256: qualification_sha256(),
        producer_commit: "a".repeat(40),
        backend_options_sha256: options_identity(&options).unwrap(),
        backend_cli_sha256: executable_sha256(&options.bootstrap.cli).unwrap(),
        executable_sha256: executable_sha256(&options.executable).unwrap(),
        staging_root_sha256: identity(&root.path()).unwrap(),
        container_mode: ContainerMode::InJob,
        acceptance_scope: AcceptanceScope::PlannedFreshInit,
        profiles: vec![],
    };
    let activation = RuntimeActivation {
        record,
        qualification: serde_json::from_slice(PIN).unwrap(),
    };
    fs::write(&options.instance_file, "local:///replacement\n").unwrap();
    options
        .environment
        .insert("FLUX_URI".into(), "local:///replacement".into());
    activation.validate_options(&options).unwrap();
    fs::write(&options.executable, "changed tool").unwrap();
    assert!(activation.validate_options(&options).is_err());
}

mod release_boundary {
    use super::*;
    use crate::adapters::flux_checks::staging::Stager;

    fn stager(fixture: &Fixture) -> Result<Stager> {
        let mut config =
            tl3_config::Config::parse(include_str!("../../../../../config/example.toml")).unwrap();
        config.flux = fixture.flux.clone();
        for repo in &mut config.github.repos {
            repo.container_image = None;
        }
        Stager::new(&config)
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn collect(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    collect(root, &entry.path(), files);
                } else {
                    files.insert(
                        entry.path().strip_prefix(root).unwrap().to_owned(),
                        fs::read(entry.path()).unwrap(),
                    );
                }
            }
        }
        let mut files = BTreeMap::new();
        collect(root, root, &mut files);
        files
    }

    fn trusted(fixture: &Fixture) -> PathBuf {
        fixture.flux.runtime_bundles[0]
            .parent()
            .unwrap()
            .join(&fixture.input.trusted_root)
    }

    fn assert_trusted(input: &backend::TreeIdentity, expected: &BTreeMap<PathBuf, Vec<u8>>) {
        let actual = snapshot(&input.root.join("trusted/tl3_harness"));
        let python = expected
            .iter()
            .filter(|(path, _)| path.extension().is_some_and(|extension| extension == "py"))
            .map(|(path, bytes)| (path.clone(), bytes.clone()))
            .collect::<BTreeMap<_, _>>();
        assert!(!python.is_empty());
        assert_eq!(actual, python);
    }

    #[test]
    #[ignore = "Flux shared-runtime fixture; run explicitly on the fleet"]
    fn a_running_controller_stages_its_qualified_release_after_producer_removal() {
        let fixture = Fixture::new();
        let controller = stager(&fixture).unwrap();
        let expected = snapshot(&trusted(&fixture));
        fs::remove_dir_all(&fixture.input.source_root).unwrap();
        let repo = crate::git_tests::Repo::new();
        for _ in 0..2 {
            let prepared = controller.prepare_request(&repo.delivery()).unwrap();
            assert_trusted(&prepared.request.tree, &expected);
        }
    }

    #[test]
    #[ignore = "Flux shared-runtime fixture; run explicitly on the fleet"]
    fn a_bundle_missing_at_startup_never_creates_an_available_stager() {
        let fixture = Fixture::new();
        let manifest = &fixture.flux.runtime_bundles[0];
        let saved = manifest.with_extension("saved");
        let parent = manifest.parent().unwrap();
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
        fs::rename(manifest, &saved).unwrap();
        fs::set_permissions(parent, fs::Permissions::from_mode(0o500)).unwrap();
        assert!(stager(&fixture).is_err());
        assert!(!fixture.flux.staging_root.exists());
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
        fs::rename(saved, manifest).unwrap();
        fs::set_permissions(parent, fs::Permissions::from_mode(0o500)).unwrap();
        // Availability requires a new, successful activation load.
        assert!(stager(&fixture).is_ok());
    }

    #[test]
    #[ignore = "Flux shared-runtime fixture; run explicitly on the fleet"]
    fn a_failed_input_copy_never_falls_back_to_the_producer() {
        let fixture = Fixture::new();
        let controller = stager(&fixture).unwrap();
        fs::write(&fixture.flux.staging_root, b"occupied").unwrap();
        let repo = crate::git_tests::Repo::new();
        assert!(controller.prepare_request(&repo.delivery()).is_err());
        assert_eq!(fs::read(&fixture.flux.staging_root).unwrap(), b"occupied");
        assert!(!repo.path.join("trusted").exists());
    }

    #[test]
    #[ignore = "Flux shared-runtime fixture; run explicitly on the fleet"]
    fn a_second_controller_load_never_touches_the_owners_input_or_bundle() {
        let fixture = Fixture::new();
        let owner = stager(&fixture).unwrap();
        let repo = crate::git_tests::Repo::new();
        let prepared = owner.prepare_request(&repo.delivery()).unwrap();
        let inputs = snapshot(&fixture.flux.staging_root);
        let bundle = snapshot(fixture.flux.runtime_bundles[0].parent().unwrap());
        let _contender = stager(&fixture).unwrap();
        assert_eq!(snapshot(&fixture.flux.staging_root), inputs);
        assert_eq!(
            snapshot(fixture.flux.runtime_bundles[0].parent().unwrap()),
            bundle
        );
        assert_trusted(&prepared.request.tree, &snapshot(&trusted(&fixture)));
    }

    #[test]
    #[ignore = "Flux shared-runtime fixture; run explicitly on the fleet"]
    fn a_current_switch_never_changes_the_qualified_module_set() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let current = fixture.root.path().join("current");
        symlink(&fixture.input.source_root, &current).unwrap();
        let controller = stager(&fixture).unwrap();
        let expected = snapshot(&trusted(&fixture));
        fs::remove_file(&current).unwrap();
        let replacement = fixture.root.path().join("replacement");
        fs::create_dir(&replacement).unwrap();
        fs::write(replacement.join("runner.py"), b"unqualified replacement").unwrap();
        symlink(replacement, &current).unwrap();
        let repo = crate::git_tests::Repo::new();
        let prepared = controller.prepare_request(&repo.delivery()).unwrap();
        assert_trusted(&prepared.request.tree, &expected);
        fs::remove_file(current).unwrap();
    }

    #[test]
    #[ignore = "Flux shared-runtime fixture; run explicitly on the fleet"]
    fn a_truncated_bundle_is_never_staged() {
        let fixture = Fixture::new();
        let controller = stager(&fixture).unwrap();
        let runner = trusted(&fixture).join("runner.py");
        fs::set_permissions(&runner, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&runner, b"truncated").unwrap();
        fs::set_permissions(runner, fs::Permissions::from_mode(0o400)).unwrap();
        let repo = crate::git_tests::Repo::new();
        assert!(controller.prepare_request(&repo.delivery()).is_err());
        assert!(!fixture.flux.staging_root.exists());
    }

    #[test]
    #[ignore = "Flux shared-runtime fixture; run explicitly on the fleet"]
    fn repeated_failed_preparations_never_publish_partial_inputs() {
        let fixture = Fixture::new();
        let controller = stager(&fixture).unwrap();
        fs::write(&fixture.flux.staging_root, b"occupied").unwrap();
        let repo = crate::git_tests::Repo::new();
        let before = snapshot(fixture.flux.runtime_bundles[0].parent().unwrap());
        for _ in 0..3 {
            assert!(controller.prepare_request(&repo.delivery()).is_err());
            assert_eq!(fs::read(&fixture.flux.staging_root).unwrap(), b"occupied");
        }
        assert_eq!(
            snapshot(fixture.flux.runtime_bundles[0].parent().unwrap()),
            before
        );
    }

    #[test]
    #[ignore = "Flux shared-runtime fixture; run explicitly on the fleet"]
    fn interrupted_preparations_preserve_owned_partial_and_completed_inputs() {
        let fixture = Fixture::new();
        let controller = stager(&fixture).unwrap();
        let partial = fixture.flux.staging_root.join("interrupted");
        fs::create_dir_all(&partial).unwrap();
        fs::write(partial.join("retained"), b"owned partial input").unwrap();
        let repo = crate::git_tests::Repo::new();
        let first = controller.prepare_request(&repo.delivery()).unwrap();
        let before = snapshot(&first.request.tree.root);
        let second = controller.prepare_request(&repo.delivery()).unwrap();
        assert_ne!(first.request.tree.root, second.request.tree.root);
        assert_eq!(snapshot(&first.request.tree.root), before);
        assert_eq!(
            fs::read(partial.join("retained")).unwrap(),
            b"owned partial input"
        );
        assert_trusted(&second.request.tree, &snapshot(&trusted(&fixture)));
    }
}


fn fresh_identity_world(
    db: &mut crate::git_tests::support::Database,
    name: &str,
) -> (crate::git_tests::Repo, crate::Controller, tl3_config::Store) {
    let repo = crate::git_tests::Repo::new();
    let mut config = tl3_config::Config::parse(include_str!("../../../../../config/example.toml")).unwrap();
    config.store = db.create(name);
    let bootstrap = crate::Bootstrap::from_config(&config).unwrap();
    let mut c = crate::Controller::open(
        tl3_store::Store::open(&config.store).unwrap(),
        crate::adapters::simulation(&repo.root.path().join("fakes")).unwrap(),
        bootstrap.clone(),
    ).unwrap();
    let repository = &bootstrap.repos[0];
    c.intake(crate::LabelledIssue {
        delivery: "first-request".into(),
        owner: repository.owner.clone(),
        repo: repository.name.clone(),
        installation: repository.installation,
        number: 42,
        title: "First request".into(),
        actor: bootstrap.rob,
        label: "tl3".into(),
        labels: vec!["tl3".into()],
    }).unwrap();
    (repo, c, config.store)
}

#[test]
fn fresh_worlds_have_distinct_effect_markers_and_flux_names() {
    let mut db = crate::git_tests::support::Database::start();
    let (_first_repo, mut first, _) = fresh_identity_world(&mut db, "world_identity_first");
    let (_second_repo, mut second, _) = fresh_identity_world(&mut db, "world_identity_second");
    let names = |c: &mut crate::Controller| {
        let snapshot = c.snapshot().unwrap();
        let delivery: crate::executor::Delivery = crate::decode_stored(snapshot.pending_effects[0].payload.clone()).unwrap();
        let key = delivery.key.clone();
        let marker = tl3_github::Client::marker(&key);
        let job = super::super::correlation(&delivery);
        let output = Path::new("/mnt/shared/flux-out/tl3").join(&job);
        (key, marker, job, output)
    };
    let a = names(&mut first);
    let b = names(&mut second);
    assert!(a.0 != b.0 && a.1 != b.1 && a.2 != b.2 && a.3 != b.3,
        "fresh worlds must distinguish effect keys, GitHub markers, Flux jobs and outputs: {a:?} versus {b:?}");
}

#[test]
fn reopened_world_keeps_durable_identity() {
    let mut db = crate::git_tests::support::Database::start();
    let (repo, mut c, settings) = fresh_identity_world(&mut db, "world_identity_reopen");
    let before = c.snapshot().unwrap();
    let id = before.rows.iter().find(|r| r.table == "kernel" && r.id == "world_id")
        .expect("initialization must durably store a world id").value.clone();
    assert!(id.as_str().is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())));
    let effects = before.pending_effects.clone();
    let bootstrap = c.bootstrap.clone();
    drop(c);
    let mut reopened = crate::Controller::open(
        tl3_store::Store::open(&settings).unwrap(),
        crate::adapters::simulation(&repo.root.path().join("fakes")).unwrap(),
        bootstrap,
    ).unwrap();
    let after = reopened.snapshot().unwrap();
    assert_eq!(after.rows.iter().find(|r| r.table == "kernel" && r.id == "world_id").unwrap().value, id);
    assert_eq!(after.pending_effects, effects);
}
